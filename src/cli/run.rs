//! `vik run [-d] [WORKFLOW]`.
//!
//! Boots the orchestrator. Foreground mode logs to both stdout and the
//! file appender; `-d` detaches via double-fork before logging is
//! installed so the surviving grandchild owns the rolling appender
//! without the original parent's stdio.

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, anyhow};
use chrono::Utc;
use clap::Parser;
use tokio_util::sync::CancellationToken;

use crate::config::{DEFAULT_HOST, ServerSchema};
use crate::daemon;
use crate::daemon::signals;
use crate::daemon::state::State;
use crate::logging::{self, LoggingGuard};
use crate::orchestrator::Orchestrator;
use crate::server::{PreparedServer, ServerAddress};
use crate::workflow::Workflow;

#[derive(Debug, Parser)]
pub struct RunArgs {
  /// Detach into the background as a daemon.
  #[arg(short = 'd', long = "detached")]
  pub detached: bool,
}

pub fn execute(workflow: Workflow, args: RunArgs) -> ExitCode {
  match run_inner(workflow, &args) {
    Ok(()) => ExitCode::SUCCESS,
    Err(err) => {
      let _ = writeln!(io::stderr(), "vik run failed: {err:#}");
      ExitCode::from(1)
    },
  }
}

fn run_inner(workflow: Workflow, args: &RunArgs) -> anyhow::Result<()> {
  workflow
    .workspace()
    .ensure_root()
    .with_context(|| format!("prepare workspace.root {}", workflow.workspace().root().display()))?;

  // Stale-state findings have to bridge the log-init boundary: the
  // pre-flight check must run before detach (the operator is still
  // looking at the foreground process), but the structured warn
  // belongs in the file appender that does not exist yet.
  let mut stale_state_note: Option<(u32, std::path::PathBuf)> = None;

  let shutdown = CancellationToken::new();

  if args.detached {
    // Reject double-detach against a live daemon before forking. Once
    // the parent `_exit`s, the operator has no foreground stderr
    // left to read — write directly to stderr here, no tracing yet.
    let state_path = workflow.workspace().service_state_file().to_path_buf();
    match State::try_read(&state_path) {
      Ok(Some(state)) if signals::pid_alive(state.pid) => {
        let _ = writeln!(
          io::stderr(),
          "vik run -d: daemon already running (pid {}, state file {})",
          state.pid,
          state_path.display(),
        );
        std::process::exit(1);
      },
      Ok(Some(state)) => {
        stale_state_note = Some((state.pid, state_path.clone()));
      },
      Ok(None) => {},
      Err(err) => {
        return Err(anyhow::Error::new(err).context(format!("read daemon state file {}", state_path.display())));
      },
    }

    let server = prepare_server(&workflow, shutdown.clone())?;

    // Detach must happen before the tracing subscriber is installed.
    // The grandchild reinstalls it below so its rolling appenders are
    // owned by the surviving process; the original parent has already
    // `_exit(0)`-ed by then.
    let log_dir = workflow.workspace().logs_dir().to_path_buf();
    if let Err(err) = std::fs::create_dir_all(&log_dir) {
      let _ = writeln!(io::stderr(), "failed to prepare log dir {}: {err}", log_dir.display());
    }
    daemon::detach(&log_dir).with_context(|| "detach daemon")?;
    run_runtime(workflow, args, shutdown, server, stale_state_note)?;
    return Ok(());
  }

  let server = prepare_server(&workflow, shutdown.clone())?;
  run_runtime(workflow, args, shutdown, server, stale_state_note)
}

fn run_runtime(
  workflow: Workflow,
  args: &RunArgs,
  shutdown: CancellationToken,
  server: Option<PreparedServer>,
  stale_state_note: Option<(u32, std::path::PathBuf)>,
) -> anyhow::Result<()> {
  let _guard = init_logging(&workflow, !args.detached)?;

  if let Some((stale_pid, stale_path)) = stale_state_note {
    tracing::info_span!("daemon").in_scope(|| {
      tracing::warn!(
        stale_pid = stale_pid as u64,
        state_file = %stale_path.display(),
        "found stale daemon state file on startup; overwriting",
      );
    });
  }

  let runtime = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .context("build tokio runtime")?;

  runtime.block_on(async move {
    let signals = signals::install_shutdown_handler_with_token(shutdown.clone())
      .map_err(|err| anyhow!("install shutdown handler: {err:#}"))?;
    let shutdown = signals.token();

    {
      let _span = tracing::info_span!("daemon").entered();

      tracing::info!(
          workflow_path = %workflow.workflow_path().display(),
          workspace_root = %workflow.workspace().root().display(),
          stage_count = workflow.stages().len() as u64,
          "starting vik",
      );
    }
    trace_http_state(server.as_ref().map(PreparedServer::address));
    // State file goes down before the orchestrator spins up so
    // lifecycle commands can already address us. Foreground runs
    // also write it — the operator may have started a foreground
    // daemon in another shell and want to manage it from elsewhere.
    let state_path = workflow.workspace().service_state_file().to_path_buf();
    write_state_file(
      &workflow,
      args,
      server.as_ref().map(PreparedServer::address),
      &state_path,
    )?;

    let mut orchestrator = Orchestrator::new(workflow);

    let exit_result = drive_runtime(&mut orchestrator, server, shutdown.clone()).await;

    // Cleanup must not fail the run: we are shutting down anyway and
    // a leftover state file just turns into a stale pid record that
    // the next `vik run -d` already knows how to recover from.
    if let Err(err) = State::remove(&state_path) {
      tracing::info_span!("daemon").in_scope(|| {
        tracing::warn!(
          path = %state_path.display(),
          error = %err,
          "failed to remove daemon state file on shutdown",
        );
      });
    }

    exit_result
  })?;

  Ok(())
}

async fn drive_runtime(
  orchestrator: &mut Orchestrator,
  server: Option<PreparedServer>,
  shutdown: CancellationToken,
) -> anyhow::Result<()> {
  let orch_token = shutdown.clone();
  let orch_future = async move {
    orchestrator
      .run(orch_token)
      .await
      .map_err(|err| anyhow!("orchestrator loop: {err:#}"))
  };

  let server_future =
    server.map(|server| async move { crate::server::run(server).await.map_err(|err| anyhow!("HTTP server: {err:#}")) });

  daemon::runtime::drive(shutdown, orch_future, server_future).await
}

fn prepare_server(workflow: &Workflow, shutdown: CancellationToken) -> anyhow::Result<Option<PreparedServer>> {
  let Some(config) = effective_server_config(workflow) else {
    return Ok(None);
  };
  let server = PreparedServer::bind(&config, shutdown).with_context(|| "prepare HTTP server")?;
  Ok(Some(server))
}

fn effective_server_config(workflow: &Workflow) -> Option<ServerSchema> {
  workflow.schema().server.clone()
}

fn trace_http_enabled(address: &ServerAddress) {
  tracing::info_span!("server").in_scope(|| {
    tracing::info!(
      bind_address = %address.bound_addr(),
      base_url = %address.url().build("/"),
      "HTTP API enabled",
    );
  });
}

fn trace_http_state(address: Option<&ServerAddress>) {
  match address {
    Some(address) => trace_http_enabled(address),
    None => trace_http_disabled(),
  }
}

fn trace_http_disabled() {
  tracing::info_span!("server").in_scope(|| {
    tracing::info!("HTTP API disabled (no server config)");
  });
}

fn init_logging(workflow: &Workflow, enable_stdout: bool) -> anyhow::Result<LoggingGuard> {
  let log_dir = workflow.workspace().logs_dir();
  logging::init(log_dir, enable_stdout)
    .with_context(|| format!("install logging subscriber writing to {}", log_dir.display()))
}

/// `command` is captured verbatim so an operator looking at a stale
/// state file can tell which invocation produced it.
fn write_state_file(
  workflow: &Workflow,
  args: &RunArgs,
  server_address: Option<&ServerAddress>,
  state_path: &Path,
) -> anyhow::Result<()> {
  let cwd = std::env::current_dir().context("read current working directory")?;
  let state = State {
    workflow_path: workflow.workflow_path().to_path_buf(),
    cwd,
    pid: std::process::id(),
    port: server_address.map(ServerAddress::port).unwrap_or(0),
    bind_address: server_address
      .map(ServerAddress::bind_address)
      .unwrap_or_else(|| DEFAULT_HOST.to_string()),
    started_at: Utc::now(),
    log_dir: workflow.workspace().logs_dir().to_path_buf(),
    sessions_dir: workflow.workspace().sessions_dir().to_path_buf(),
    command: format_command(workflow, args),
  };
  state
    .write(state_path)
    .with_context(|| format!("write daemon state file to {}", state_path.display()))?;
  tracing::info_span!("daemon").in_scope(|| {
    tracing::info!(
      state_file = %state_path.display(),
      pid = state.pid,
      port = state.port as u64,
      "daemon state file written",
    );
  });
  Ok(())
}

fn format_command(workflow: &Workflow, args: &RunArgs) -> String {
  let mut parts = vec!["vik".to_string(), "run".to_string()];
  if args.detached {
    parts.push("-d".into());
  }
  parts.push(workflow.workflow_path().display().to_string());
  parts.join(" ")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::ServerSchema;

  #[test]
  fn format_command_records_detached_and_workflow_path() {
    let workflow = Workflow::builder().workflow_path("/tmp/vik-cli-tests/workflow.yml").build();
    let args = RunArgs { detached: true };

    assert_eq!(
      format_command(&workflow, &args),
      "vik run -d /tmp/vik-cli-tests/workflow.yml"
    );
  }

  #[test]
  fn effective_server_config_keeps_http_disabled_when_server_is_missing() {
    let workflow = Workflow::builder().build();

    let config = effective_server_config(&workflow);

    assert!(config.is_none());
  }

  #[test]
  fn effective_server_config_uses_workflow_server() {
    let mut server = ServerSchema::default();
    server.host = "0.0.0.0".into();
    server.port = 9000;
    server.https = true;
    server.domain = Some("example.local".into());
    let workflow = Workflow::builder().server(server).build();

    let config = effective_server_config(&workflow).expect("enabled");

    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 9000);
    assert!(config.https);
    assert_eq!(config.domain.as_deref(), Some("example.local"));
  }

  #[test]
  fn prepare_server_reports_bind_error_before_runtime_start() {
    let occupied = std::net::TcpListener::bind((DEFAULT_HOST, 0)).expect("occupy port");
    let mut server = ServerSchema::default();
    server.port = occupied.local_addr().expect("occupied addr").port();
    let workflow = Workflow::builder().server(server).build();

    let err = match prepare_server(&workflow, CancellationToken::new()) {
      Ok(_) => panic!("bind must fail"),
      Err(err) => err,
    };

    assert!(format!("{err:#}").contains("prepare HTTP server"));
  }

  #[test]
  fn prepared_random_port_server_is_recorded_in_state_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow = Workflow::builder()
      .workflow_path(temp.path().join("workflow.yml"))
      .workspace_root(temp.path())
      .server(ServerSchema::default())
      .build();
    let args = RunArgs { detached: false };
    let state_path = temp.path().join("state.json");
    let shutdown = CancellationToken::new();
    let server = prepare_server(&workflow, shutdown.clone())
      .expect("prepare server")
      .expect("server enabled");

    write_state_file(&workflow, &args, Some(server.address()), &state_path).expect("state file written");
    let state = State::try_read(&state_path).expect("state reads").expect("state exists");

    assert_eq!(state.bind_address, "127.0.0.1");
    assert_ne!(state.port, 0);
    assert_eq!(state.port, server.address().port());

    shutdown.cancel();
  }
}
