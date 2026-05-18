//! `vik run [-d] [--port N] [WORKFLOW]`.
//!
//! Boots the orchestrator. Foreground mode logs to both stdout and the
//! file appender; `-d` detaches via double-fork before logging is
//! installed so the surviving grandchild owns the rolling appender
//! without the original parent's stdio.

use std::io::{self, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, anyhow};
use chrono::Utc;
use clap::Parser;
use tokio_util::sync::CancellationToken;

use crate::daemon;
use crate::daemon::signals;
use crate::daemon::state::State;
use crate::logging::{self, LoggingGuard};
use crate::orchestrator::Orchestrator;
use crate::workflow::Workflow;

#[derive(Debug, Parser)]
pub struct RunArgs {
  /// TCP port for the HTTP API. Omit to run without the HTTP API.
  #[arg(long)]
  pub port: Option<u16>,

  /// Bind address for the HTTP API. Only meaningful with `--port`.
  #[arg(long, default_value = "127.0.0.1")]
  pub bind_address: String,

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

    // Detach must happen before the tracing subscriber is installed.
    // The grandchild reinstalls it below so its rolling appenders are
    // owned by the surviving process; the original parent has already
    // `_exit(0)`-ed by then.
    let log_dir = workflow.workspace().logs_dir().to_path_buf();
    if let Err(err) = std::fs::create_dir_all(&log_dir) {
      let _ = writeln!(io::stderr(), "failed to prepare log dir {}: {err}", log_dir.display());
    }
    daemon::detach(&log_dir).with_context(|| "detach daemon")?;
  }

  let _guard = init_logging(&workflow, !args.detached)?;

  if let Some((stale_pid, stale_path)) = stale_state_note {
    tracing::error_span!("daemon").in_scope(|| {
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
    let signals = daemon::install_shutdown_handler().map_err(|err| anyhow!("install shutdown handler: {err:#}"))?;
    let shutdown = signals.token();

    {
      let _span = tracing::error_span!("daemon").entered();

      tracing::info!(
          workflow_path = %workflow.workflow_path().display(),
          workspace_root = %workflow.workspace().root().display(),
          stage_count = workflow.stages().len() as u64,
          "starting vik",
      );
    }
    // State file goes down before the orchestrator spins up so
    // lifecycle commands can already address us. Foreground runs
    // also write it — the operator may have started a foreground
    // daemon in another shell and want to manage it from elsewhere.
    let state_path = workflow.workspace().service_state_file().to_path_buf();
    let bind_address = resolve_bind_address(args)?;
    write_state_file(&workflow, args, bind_address.as_ref(), &state_path)?;

    let mut orchestrator = Orchestrator::new(workflow);

    let exit_result = drive_runtime(&mut orchestrator, bind_address, shutdown.clone()).await;

    // Cleanup must not fail the run: we are shutting down anyway and
    // a leftover state file just turns into a stale pid record that
    // the next `vik run -d` already knows how to recover from.
    if let Err(err) = State::remove(&state_path) {
      tracing::error_span!("daemon").in_scope(|| {
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
  bind_address: Option<SocketAddr>,
  shutdown: CancellationToken,
) -> anyhow::Result<()> {
  let orch_token = shutdown.clone();
  let orch_future = async move {
    orchestrator
      .run(orch_token)
      .await
      .map_err(|err| anyhow!("orchestrator loop: {err:#}"))
  };

  match bind_address {
    Some(addr) => {
      trace_http_enabled(addr);
      todo!("vik run with --port is not implemented yet");
    },
    None => {
      trace_http_disabled();
      super::shutdown::graceful(shutdown, orch_future).await
    },
  }
}

fn trace_http_enabled(addr: SocketAddr) {
  tracing::error_span!("server").in_scope(|| {
    tracing::info!(
      bind_address = %addr,
      "HTTP API enabled",
    );
  });
}

fn trace_http_disabled() {
  tracing::error_span!("server").in_scope(|| {
    tracing::info!("HTTP API disabled (no --port)");
  });
}

fn resolve_bind_address(args: &RunArgs) -> anyhow::Result<Option<SocketAddr>> {
  let Some(port) = args.port else {
    return Ok(None);
  };
  let ip: IpAddr = args
    .bind_address
    .parse()
    .with_context(|| format!("parse --bind-address `{}`", args.bind_address))?;
  Ok(Some(SocketAddr::new(ip, port)))
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
  bind_address: Option<&SocketAddr>,
  state_path: &Path,
) -> anyhow::Result<()> {
  let cwd = std::env::current_dir().context("read current working directory")?;
  let state = State {
    workflow_path: workflow.workflow_path().to_path_buf(),
    cwd,
    pid: std::process::id(),
    port: bind_address.map(|a| a.port()).unwrap_or(0),
    bind_address: bind_address
      .map(|a| a.ip().to_string())
      .unwrap_or_else(|| args.bind_address.clone()),
    started_at: Utc::now(),
    log_dir: workflow.workspace().logs_dir().to_path_buf(),
    sessions_dir: workflow.workspace().sessions_dir().to_path_buf(),
    command: format_command(workflow, args),
  };
  state
    .write(state_path)
    .with_context(|| format!("write daemon state file to {}", state_path.display()))?;
  tracing::error_span!("daemon").in_scope(|| {
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
  if let Some(port) = args.port {
    parts.push(format!("--port {port}"));
  }
  if args.bind_address != "127.0.0.1" {
    parts.push(format!("--bind-address {}", args.bind_address));
  }
  parts.push(workflow.workflow_path().display().to_string());
  parts.join(" ")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::logging::tests::{CaptureLayer, captured_event};
  use tracing_subscriber::{Registry, layer::SubscriberExt};

  #[test]
  fn format_command_records_detached_http_args_and_workflow_path() {
    let workflow = Workflow::builder().workflow_path("/tmp/vik-cli-tests/workflow.yml").build();
    let args = RunArgs {
      port: Some(8123),
      bind_address: "0.0.0.0".to_string(),
      detached: true,
    };

    assert_eq!(
      format_command(&workflow, &args),
      "vik run -d --port 8123 --bind-address 0.0.0.0 /tmp/vik-cli-tests/workflow.yml"
    );
  }

  #[test]
  fn resolve_bind_address_builds_socket_when_port_is_set() {
    let args = RunArgs {
      port: Some(9000),
      bind_address: "::1".to_string(),
      detached: false,
    };

    let addr = resolve_bind_address(&args).expect("valid bind address");

    assert_eq!(addr, Some("[::1]:9000".parse().expect("socket address")));
  }

  #[test]
  fn resolve_bind_address_skips_bind_parsing_without_port() {
    let args = RunArgs {
      port: None,
      bind_address: "not an ip address".to_string(),
      detached: false,
    };

    let addr = resolve_bind_address(&args).expect("bind address is unused");

    assert_eq!(addr, None);
  }

  #[test]
  fn write_state_file_logs_inside_daemon_span() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);
    let _default = tracing::subscriber::set_default(subscriber);

    let temp = tempfile::tempdir().expect("tempdir");
    let workflow = Workflow::builder()
      .workflow_path(temp.path().join("workflow.yml"))
      .workspace_root(temp.path())
      .build();
    let args = RunArgs {
      port: None,
      bind_address: "127.0.0.1".to_string(),
      detached: false,
    };
    let state_path = temp.path().join("state.json");

    write_state_file(&workflow, &args, None, &state_path).expect("state file written");

    let events = events.lock().expect("events mutex");
    let event = captured_event(&events, "daemon state file written");
    assert_eq!(event["spans"][0]["name"], "daemon");
    assert!(event.get("phase").is_none());
  }

  #[test]
  fn http_status_logs_inside_server_span() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);
    let _default = tracing::subscriber::set_default(subscriber);

    trace_http_enabled("127.0.0.1:9000".parse().expect("socket address"));
    trace_http_disabled();

    let events = events.lock().expect("events mutex");
    let enabled = captured_event(&events, "HTTP API enabled");
    let disabled = captured_event(&events, "HTTP API disabled (no --port)");
    assert_eq!(enabled["spans"][0]["name"], "server");
    assert_eq!(disabled["spans"][0]["name"], "server");
    assert!(events.iter().all(|event| event.get("phase").is_none()));
  }
}
