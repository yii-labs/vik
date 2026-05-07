//! Daemon-lifecycle subcommands: `status`, `stop`, `restart`, `uninstall`.
//!
//! Thin shells around [`crate::daemon::lifecycle`] — heavy lifting (state
//! file I/O, pid liveness, SIGTERM polling) lives there. This file is
//! parse + render + exit code only.

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::anyhow;
use clap::Parser;

use crate::daemon::lifecycle::{RestartOutcome, STOP_TIMEOUT, StatusReport};
use crate::daemon::{self};
use crate::workflow::Workflow;

/// Args shared by `status`, `stop`, `uninstall`.
#[derive(Debug, Parser)]
pub struct LifecycleArgs {}

#[derive(Debug, Parser)]
pub struct RestartArgs {
  /// TCP port for the new daemon's HTTP API. Omit to start without
  /// the HTTP API.
  #[arg(long)]
  pub port: Option<u16>,

  /// Bind address for the HTTP API.
  #[arg(long, default_value = "127.0.0.1")]
  pub bind_address: String,
}

/// Exit 0 for `running`, `stale`, and `not installed` — the operator
/// gets the answer in stdout. Exit 1 only for actual I/O failures, so
/// scripting around `vik status` does not need to special-case the
/// "absent daemon" case.
pub fn status(workflow: Workflow, _args: LifecycleArgs) -> ExitCode {
  match status_inner(&workflow) {
    Ok(report) => {
      render_status(&report);
      ExitCode::SUCCESS
    },
    Err(err) => {
      let _ = writeln!(io::stderr(), "vik status failed: {err:#}");
      ExitCode::from(1)
    },
  }
}

fn status_inner(workflow: &Workflow) -> anyhow::Result<StatusReport> {
  let path = workflow.workspace().service_state_file().to_path_buf();
  daemon::lifecycle::status(&path).map_err(|err| anyhow!(err))
}

fn render_status(report: &StatusReport) {
  let stdout = io::stdout();
  let mut handle = stdout.lock();
  let _ = writeln!(handle, "status: {}", report.status.as_str());
  if let Some(state) = &report.state {
    let _ = writeln!(handle, "state_file: {}", report.state_path.display());
    let _ = writeln!(handle, "pid: {}", state.pid);
    let _ = writeln!(handle, "bind_address: {}:{}", state.bind_address, state.port);
    let _ = writeln!(handle, "started_at: {}", state.started_at);
    let _ = writeln!(handle, "log_dir: {}", state.log_dir.display());
    let _ = writeln!(handle, "sessions_dir: {}", state.sessions_dir.display());
    let _ = writeln!(handle, "workflow_path: {}", state.workflow_path.display());
    let _ = writeln!(handle, "command: {}", state.command);
  } else {
    let _ = writeln!(handle, "state_file: {}", report.state_path.display());
  }
}

pub fn stop(workflow: Workflow, _args: LifecycleArgs) -> ExitCode {
  match stop_inner(&workflow) {
    Ok(()) => ExitCode::SUCCESS,
    Err(err) => {
      let _ = writeln!(io::stderr(), "vik stop failed: {err:#}");
      ExitCode::from(1)
    },
  }
}

fn stop_inner(workflow: &Workflow) -> anyhow::Result<()> {
  let path = workflow.workspace().service_state_file().to_path_buf();
  daemon::lifecycle::stop(&path, STOP_TIMEOUT).map_err(|err| anyhow!(err))?;
  let _ = writeln!(io::stdout(), "daemon stopped");
  Ok(())
}

/// Implemented as "stop, then run -d" so both entry points share one
/// daemon-startup path. Always restarts silently when no daemon was
/// running — operators can still inspect what happened via the
/// "no daemon was running" message.
pub fn restart(workflow: Workflow, args: RestartArgs) -> ExitCode {
  match restart_stop_phase(&workflow) {
    Ok(RestartOutcome::Stopped) => {
      let _ = writeln!(io::stdout(), "daemon stopped; starting a fresh one");
    },
    Ok(RestartOutcome::NotRunning) => {
      let _ = writeln!(io::stdout(), "no daemon was running; starting one");
    },
    Err(err) => {
      let _ = writeln!(io::stderr(), "vik restart failed: {err:#}");
      return ExitCode::from(1);
    },
  }

  super::run::execute(
    workflow,
    super::run::RunArgs {
      port: args.port,
      bind_address: args.bind_address,
      detached: true,
    },
  )
}

fn restart_stop_phase(workflow: &Workflow) -> anyhow::Result<RestartOutcome> {
  let path = workflow.workspace().service_state_file().to_path_buf();
  daemon::lifecycle::restart_stop_phase(&path, STOP_TIMEOUT).map_err(|err| anyhow!(err))
}

pub fn uninstall(workflow: Workflow, _args: LifecycleArgs) -> ExitCode {
  match uninstall_inner(&workflow) {
    Ok(()) => ExitCode::SUCCESS,
    Err(err) => {
      let _ = writeln!(io::stderr(), "vik uninstall failed: {err:#}");
      ExitCode::from(1)
    },
  }
}

fn uninstall_inner(workflow: &Workflow) -> anyhow::Result<()> {
  let path = workflow.workspace().service_state_file().to_path_buf();
  daemon::lifecycle::uninstall(&path, STOP_TIMEOUT).map_err(|err| anyhow!(err))?;
  let _ = writeln!(io::stdout(), "daemon uninstalled (state file removed if present)");
  Ok(())
}
