//! `vik run` daemon entry point.
//!
//! The CLI parses flags and calls this module. The daemon layer owns startup
//! order, state-file lifecycle, HTTP server binding, and orchestrator runtime
//! driving.

use std::io::{self, Write};

use anyhow::{Context, anyhow};
use tokio_util::sync::CancellationToken;

use super::state::StateManager;
use super::{detach, runtime, signals};
use crate::logging::{self, LoggingGuard};
use crate::orchestrator::Orchestrator;
use crate::server::{PreparedServer, ServerAddress};
use crate::workflow::Workflow;

pub fn run(workflow: Workflow, detached: bool) -> anyhow::Result<()> {
  workflow
    .workspace()
    .ensure_root()
    .with_context(|| format!("prepare workspace.root {}", workflow.workspace().root().display()))?;

  let state_manager = StateManager::new(workflow.workspace().service_state_file().to_path_buf());

  // Stale-state findings have to bridge the log-init boundary: the
  // pre-flight check must run before detach (the operator is still
  // looking at the foreground process), but the structured warn
  // belongs in the file appender that does not exist yet.
  let mut stale_state_note: Option<(u32, std::path::PathBuf)> = None;

  let shutdown = CancellationToken::new();

  if detached {
    // Reject double-detach against a live daemon before forking. Once
    // the parent `_exit`s, the operator has no foreground stderr
    // left to read; write directly to stderr here, no tracing yet.
    match state_manager.read() {
      Ok(Some(state)) if signals::pid_alive(state.pid) => {
        let _ = writeln!(
          io::stderr(),
          "vik run -d: daemon already running (pid {}, state file {})",
          state.pid,
          state_manager.path().display(),
        );
        std::process::exit(1);
      },
      Ok(Some(state)) => {
        stale_state_note = Some((state.pid, state_manager.path().to_path_buf()));
      },
      Ok(None) => {},
      Err(err) => {
        return Err(
          anyhow::Error::new(err).context(format!("read daemon state file {}", state_manager.path().display())),
        );
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
    detach(&log_dir).with_context(|| "detach daemon")?;
    run_runtime(workflow, detached, shutdown, server, stale_state_note, state_manager)?;
    return Ok(());
  }

  let server = prepare_server(&workflow, shutdown.clone())?;
  run_runtime(workflow, detached, shutdown, server, stale_state_note, state_manager)
}

fn run_runtime(
  workflow: Workflow,
  detached: bool,
  shutdown: CancellationToken,
  server: PreparedServer,
  stale_state_note: Option<(u32, std::path::PathBuf)>,
  state_manager: StateManager,
) -> anyhow::Result<()> {
  let _guard = init_logging(&workflow, !detached)?;

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
    trace_http_enabled(server.address());

    // State file goes down before the orchestrator spins up so
    // lifecycle commands can already address us. Foreground runs
    // also write it: the operator may have started a foreground
    // daemon in another shell and want to manage it from elsewhere.
    state_manager
      .write_runtime_state(
        &workflow,
        server.address().port(),
        server.address().bind_address(),
        format_command(&workflow, detached),
      )
      .with_context(|| format!("write daemon state file to {}", state_manager.path().display()))?;

    let mut orchestrator = Orchestrator::new(workflow);

    let exit_result = drive_runtime(&mut orchestrator, server, shutdown.clone()).await;

    // Cleanup must not fail the run: we are shutting down anyway and
    // a leftover state file just turns into a stale pid record that
    // the next `vik run -d` already knows how to recover from.
    if let Err(err) = state_manager.remove() {
      tracing::info_span!("daemon").in_scope(|| {
        tracing::warn!(
          path = %state_manager.path().display(),
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
  server: PreparedServer,
  shutdown: CancellationToken,
) -> anyhow::Result<()> {
  let orch_token = shutdown.clone();
  let orch_future = async move {
    orchestrator
      .run(orch_token)
      .await
      .map_err(|err| anyhow!("orchestrator loop: {err:#}"))
  };

  let server_future = async move { crate::server::run(server).await.map_err(|err| anyhow!("HTTP server: {err:#}")) };

  runtime::drive(shutdown, orch_future, Some(server_future)).await
}

fn prepare_server(workflow: &Workflow, shutdown: CancellationToken) -> anyhow::Result<PreparedServer> {
  let config = workflow.schema().server.clone().unwrap_or_default();
  PreparedServer::bind(&config, shutdown).with_context(|| "prepare HTTP server")
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

fn init_logging(workflow: &Workflow, enable_stdout: bool) -> anyhow::Result<LoggingGuard> {
  let log_dir = workflow.workspace().logs_dir();
  logging::init(log_dir, enable_stdout)
    .with_context(|| format!("install logging subscriber writing to {}", log_dir.display()))
}

fn format_command(workflow: &Workflow, detached: bool) -> String {
  let mut parts = vec!["vik".to_string(), "run".to_string()];
  if detached {
    parts.push("-d".into());
  }
  parts.push(workflow.workflow_path().display().to_string());
  parts.join(" ")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::ServerSchema;
  use crate::logging::tests::{capture_events, captured_event};

  #[test]
  fn format_command_records_detached_and_workflow_path() {
    let workflow = Workflow::builder().workflow_path("/tmp/vik-cli-tests/workflow.yml").build();

    assert_eq!(
      format_command(&workflow, true),
      "vik run -d /tmp/vik-cli-tests/workflow.yml"
    );
  }

  #[test]
  fn prepare_server_uses_default_config_when_server_is_missing() {
    let workflow = Workflow::builder().build();
    let shutdown = CancellationToken::new();

    let server = prepare_server(&workflow, shutdown.clone()).expect("server enabled");

    assert_eq!(server.address().bind_address(), "127.0.0.1");
    assert_ne!(server.address().port(), 0);
    shutdown.cancel();
  }

  #[test]
  fn prepare_server_uses_workflow_server() {
    let mut config = ServerSchema::default();
    config.https = true;
    config.domain = Some("example.local".into());
    let workflow = Workflow::builder().server(config).build();
    let shutdown = CancellationToken::new();

    let server = prepare_server(&workflow, shutdown.clone()).expect("server enabled");

    assert_eq!(server.address().url().build("/status"), "https://example.local/status");
    shutdown.cancel();
  }

  #[test]
  fn prepare_server_reports_bind_error_before_runtime_start() {
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("occupy port");
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
  fn http_status_logs_inside_server_span() {
    let (events, _capture) = capture_events();

    let address = ServerAddress::new(false, None, "127.0.0.1:9000".parse().expect("socket address"));
    trace_http_enabled(&address);

    let events = events.lock().expect("events mutex");
    let enabled = captured_event(&events, "HTTP API enabled");
    assert_eq!(enabled["spans"][0]["name"], "server");
    assert!(events.iter().all(|event| event.get("phase").is_none()));
  }
}
