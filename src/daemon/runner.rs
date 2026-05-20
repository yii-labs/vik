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
use crate::logging;
use crate::orchestrator::Orchestrator;
use crate::server::ServerConfig;
use crate::workflow::Workflow;

pub fn run(workflow: Workflow, detached: bool) -> anyhow::Result<()> {
  workflow
    .workspace()
    .ensure_root()
    .with_context(|| format!("prepare workspace.root {}", workflow.workspace().root().display()))?;

  let state_manager = StateManager::new(workflow.workspace().service_state_file().to_path_buf());

  let shutdown = CancellationToken::new();

  // Stale-state findings have to bridge the log-init boundary: the
  // pre-flight check must run before detach (the operator is still
  // looking at the foreground process), but the structured warn
  // belongs in the file appender that does not exist yet.
  let stale_state_note = if detached {
    match state_manager.assert_not_running() {
      Ok(stale) => stale.map(|stale| (stale.pid, stale.path)),
      Err(crate::daemon::StateError::AlreadyRunning { pid, path }) => {
        let _ = writeln!(
          io::stderr(),
          "vik run -d: daemon already running (pid {}, state file {})",
          pid,
          path.display(),
        );
        std::process::exit(1);
      },
      Err(err) => {
        return Err(
          anyhow::Error::new(err).context(format!("read daemon state file {}", state_manager.path().display())),
        );
      },
    }
  } else {
    None
  };

  let (server_config, server_future) =
    crate::server::run(&workflow, shutdown.clone()).with_context(|| "prepare HTTP server")?;

  if detached {
    // Detach must happen before the tracing subscriber is installed.
    // The grandchild reinstalls it below so its rolling appenders are
    // owned by the surviving process; the original parent has already
    // `_exit(0)`-ed by then.
    let log_dir = workflow.workspace().logs_dir().to_path_buf();
    if let Err(err) = std::fs::create_dir_all(&log_dir) {
      let _ = writeln!(io::stderr(), "failed to prepare log dir {}: {err}", log_dir.display());
    }
    detach(&log_dir).with_context(|| "detach daemon")?;
    run_runtime(
      workflow,
      detached,
      shutdown,
      server_config,
      server_future,
      stale_state_note,
      state_manager,
    )?;
    return Ok(());
  }

  run_runtime(
    workflow,
    detached,
    shutdown,
    server_config,
    server_future,
    stale_state_note,
    state_manager,
  )
}

fn run_runtime<S>(
  workflow: Workflow,
  detached: bool,
  shutdown: CancellationToken,
  server_config: ServerConfig,
  server: S,
  stale_state_note: Option<(u32, std::path::PathBuf)>,
  state_manager: StateManager,
) -> anyhow::Result<()>
where
  S: std::future::Future<Output = Result<(), crate::server::ServerError>>,
{
  let log_dir = workflow.workspace().logs_dir();
  let _guard = logging::init(log_dir, !detached)
    .with_context(|| format!("install logging subscriber writing to {}", log_dir.display()))?;

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
    trace_http_enabled(&server_config);

    // State file goes down before the orchestrator spins up so
    // lifecycle commands can already address us. Foreground runs
    // also write it: the operator may have started a foreground
    // daemon in another shell and want to manage it from elsewhere.
    state_manager
      .write_runtime_state(
        &workflow,
        server_config.port(),
        server_config.bind_address(),
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

async fn drive_runtime<S>(orchestrator: &mut Orchestrator, server: S, shutdown: CancellationToken) -> anyhow::Result<()>
where
  S: std::future::Future<Output = Result<(), crate::server::ServerError>>,
{
  let orch_token = shutdown.clone();
  let orch_future = async move {
    orchestrator
      .run(orch_token)
      .await
      .map_err(|err| anyhow!("orchestrator loop: {err:#}"))
  };

  let server_future = async move { server.await.map_err(|err| anyhow!("HTTP server: {err:#}")) };

  runtime::drive(shutdown, orch_future, Some(server_future)).await
}

fn trace_http_enabled(address: &ServerConfig) {
  tracing::info_span!("server").in_scope(|| {
    tracing::info!(
      bind_address = %address.bound_addr(),
      base_url = %address.url().build("/"),
      "HTTP API enabled",
    );
  });
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
  fn http_status_logs_inside_server_span() {
    let (events, _capture) = capture_events();

    let address = ServerConfig::new(false, None, "127.0.0.1:9000".parse().expect("socket address"));
    trace_http_enabled(&address);

    let events = events.lock().expect("events mutex");
    let enabled = captured_event(&events, "HTTP API enabled");
    assert_eq!(enabled["spans"][0]["name"], "server");
    assert!(events.iter().all(|event| event.get("phase").is_none()));
  }
}
