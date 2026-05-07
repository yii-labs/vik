//! Lifecycle verbs: `status`, `stop`, `restart`, `uninstall`.
//!
//! Each verb is a pure function over [`State`] + a pid liveness probe;
//! the CLI layer composes them and renders output. None of these
//! functions spawn subprocesses — `restart` is implemented as
//! "stop, then call `cli::run::execute` again," wired in `cli/`.

use std::path::Path;
use std::time::{Duration, Instant};

use thiserror::Error;

use super::state::{State, StateError};

/// Matches the daemon-side graceful-shutdown budget. Mismatch would
/// mean `vik stop` either hangs past the daemon's exit or gives up
/// before the daemon finishes.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// Granular enough to catch sub-second graceful exits, sparse enough
/// not to flood `kill(pid, 0)` calls during the wait.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Error)]
pub enum LifecycleError {
  #[error(transparent)]
  State(#[from] StateError),

  /// Distinct from `Ok(())` so operator scripts can tell `vik stop` on
  /// a missing daemon apart from a successful stop.
  #[error("no daemon state file for {path}")]
  NotInstalled { path: std::path::PathBuf },

  /// Usually EPERM (pid taken over by another user) or ESRCH (pid was
  /// already gone). The caller may still want to inspect liveness
  /// before treating as fatal.
  #[error("failed to signal daemon pid {pid}: {source}")]
  Signal {
    pid: u32,
    #[source]
    source: std::io::Error,
  },

  #[error("daemon pid {pid} did not exit within {timeout_ms} ms")]
  StopTimedOut { pid: u32, timeout_ms: u128 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusState {
  Running,
  /// State file present but pid is gone. Operators can `vik uninstall`
  /// to clean up.
  Stale,
  /// `vik status` exits 0 in this state so `vik status || vik run -d`
  /// works without conditionals.
  NotInstalled,
}

impl StatusState {
  pub fn as_str(self) -> &'static str {
    match self {
      StatusState::Running => "running",
      StatusState::Stale => "stale",
      StatusState::NotInstalled => "not installed",
    }
  }
}

#[derive(Debug)]
pub struct StatusReport {
  pub status: StatusState,
  pub state_path: std::path::PathBuf,
  pub state: Option<State>,
}

#[derive(Debug)]
pub enum RestartOutcome {
  Stopped,
  NotRunning,
}

pub fn status(state_path: &Path) -> Result<StatusReport, LifecycleError> {
  match State::try_read(state_path)? {
    Some(state) => {
      let alive = pid_alive(state.pid);
      let status = if alive {
        StatusState::Running
      } else {
        StatusState::Stale
      };
      Ok(StatusReport {
        status,
        state_path: state_path.to_path_buf(),
        state: Some(state),
      })
    },
    None => Ok(StatusReport {
      status: StatusState::NotInstalled,
      state_path: state_path.to_path_buf(),
      state: None,
    }),
  }
}

pub fn stop(state_path: &Path, deadline: Duration) -> Result<(), LifecycleError> {
  let state = match State::try_read(state_path)? {
    Some(s) => s,
    None => {
      return Err(LifecycleError::NotInstalled {
        path: state_path.to_path_buf(),
      });
    },
  };
  stop_with_state(&state, state_path, deadline)
}

/// Shared between `stop` and `restart` so we do not re-read the state
/// file twice during a restart.
fn stop_with_state(state: &State, state_path: &Path, deadline: Duration) -> Result<(), LifecycleError> {
  if !pid_alive(state.pid) {
    // Stale path: clean up the file so future `status` calls see a
    // truthful "not installed" rather than the zombie record.
    State::remove(state_path)?;
    return Ok(());
  }

  send_sigterm(state.pid)?;

  let started = Instant::now();
  while started.elapsed() < deadline {
    if !pid_alive(state.pid) {
      State::remove(state_path)?;
      return Ok(());
    }
    std::thread::sleep(POLL_INTERVAL);
  }

  Err(LifecycleError::StopTimedOut {
    pid: state.pid,
    timeout_ms: deadline.as_millis(),
  })
}

pub fn restart_stop_phase(state_path: &Path, deadline: Duration) -> Result<RestartOutcome, LifecycleError> {
  let state = match State::try_read(state_path)? {
    Some(s) => s,
    None => return Ok(RestartOutcome::NotRunning),
  };
  if !pid_alive(state.pid) {
    State::remove(state_path)?;
    return Ok(RestartOutcome::NotRunning);
  }
  stop_with_state(&state, state_path, deadline)?;
  Ok(RestartOutcome::Stopped)
}

/// No-op when the file is missing — operator scripts can call
/// `vik uninstall` unconditionally during teardown.
pub fn uninstall(state_path: &Path, deadline: Duration) -> Result<(), LifecycleError> {
  if let Some(state) = State::try_read(state_path)? {
    if pid_alive(state.pid) {
      stop_with_state(&state, state_path, deadline)?;
    } else {
      State::remove(state_path)?;
    }
  }
  Ok(())
}

fn send_sigterm(pid: u32) -> Result<(), LifecycleError> {
  super::signals::send_sigterm(pid).map_err(|source| LifecycleError::Signal { pid, source })
}

pub fn pid_alive(pid: u32) -> bool {
  super::signals::pid_alive(pid)
}

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::TempDir;

  fn sample_state(pid: u32) -> State {
    State {
      workflow_path: "/tmp/workflow.yml".into(),
      cwd: "/tmp".into(),
      pid,
      port: 3000,
      bind_address: "127.0.0.1".into(),
      started_at: "2026-05-09T10:00:00Z".parse().unwrap(),
      log_dir: "/tmp/.vik/logs".into(),
      sessions_dir: "/tmp/.vik/sessions".into(),
      command: "vik run -d".into(),
    }
  }

  #[test]
  fn status_reports_not_installed_when_file_missing() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    let report = status(&path).expect("status ok");
    assert_eq!(report.status, StatusState::NotInstalled);
    assert!(report.state.is_none());
  }

  #[test]
  fn status_reports_stale_when_pid_dead() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    // 2^31-2 is well outside any realistic pid space; pid 1 would
    // be flaky because init is always alive.
    let dead = 2_147_483_646u32;
    sample_state(dead).write(&path).expect("write");
    let report = status(&path).expect("status ok");
    assert_eq!(report.status, StatusState::Stale);
    assert_eq!(report.state.unwrap().pid, dead);
  }

  #[cfg(unix)]
  #[test]
  fn status_reports_running_for_own_pid() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    let me = std::process::id();
    sample_state(me).write(&path).expect("write");
    let report = status(&path).expect("status ok");
    assert_eq!(report.status, StatusState::Running);
  }

  #[test]
  fn stop_returns_not_installed_on_missing_file() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    let err = stop(&path, Duration::from_millis(50)).expect_err("must fail");
    assert!(matches!(err, LifecycleError::NotInstalled { .. }));
  }

  #[test]
  fn stop_removes_stale_file_without_signal() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    let dead = 2_147_483_646u32;
    sample_state(dead).write(&path).expect("write");
    stop(&path, Duration::from_millis(50)).expect("stale cleans up");
    assert!(!path.exists());
  }

  #[test]
  fn uninstall_is_noop_when_missing() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    uninstall(&path, Duration::from_millis(50)).expect("noop ok");
  }

  #[test]
  fn uninstall_removes_stale_file() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    let dead = 2_147_483_646u32;
    sample_state(dead).write(&path).expect("write");
    uninstall(&path, Duration::from_millis(50)).expect("ok");
    assert!(!path.exists());
  }

  #[test]
  fn restart_stop_phase_reports_not_running_when_missing() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    let outcome = restart_stop_phase(&path, Duration::from_millis(50)).expect("ok");
    assert!(matches!(outcome, RestartOutcome::NotRunning));
  }

  #[test]
  fn restart_stop_phase_reports_not_running_when_stale() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    let dead = 2_147_483_646u32;
    sample_state(dead).write(&path).expect("write");
    let outcome = restart_stop_phase(&path, Duration::from_millis(50)).expect("ok");
    assert!(matches!(outcome, RestartOutcome::NotRunning));
    assert!(!path.exists(), "stale file removed by restart stop phase");
  }
}
