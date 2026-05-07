//! Shutdown-signal wiring.
//!
//! [`install_shutdown_handler`] installs handlers for SIGINT/SIGTERM
//! (Unix) or Ctrl-C (Windows). The first signal trips the returned
//! token; a second one in the same process aborts via `exit(130)` so
//! an impatient operator can force termination. SIGHUP is observed
//! and logged but otherwise ignored — no config-reload feature.

use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

#[cfg(test)]
mod tests;

/// ESRCH ("no such process") maps to `Ok(())` so `vik stop` against a
/// stale state file does not error on the signal step itself.
pub fn send_sigterm(pid: u32) -> std::io::Result<()> {
  #[cfg(unix)]
  {
    unix::send_sigterm(pid)
  }
  #[cfg(windows)]
  {
    windows::send_sigterm(pid)
  }
}

/// `true` when we could in principle signal `pid` (even without
/// permission). EPERM counts as alive on Unix.
pub fn pid_alive(pid: u32) -> bool {
  #[cfg(unix)]
  {
    unix::pid_alive(pid)
  }
  #[cfg(windows)]
  {
    windows::pid_alive(pid)
  }
}

#[derive(Debug, Error)]
pub enum SignalError {
  #[error("failed to install OS signal handler: {0}")]
  Install(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct ShutdownSignals {
  token: CancellationToken,
}

impl ShutdownSignals {
  pub fn token(&self) -> CancellationToken {
    self.token.clone()
  }
}

pub fn install_shutdown_handler() -> Result<ShutdownSignals, SignalError> {
  let token = CancellationToken::new();
  #[cfg(unix)]
  {
    unix::install(token.clone())?;
  }
  #[cfg(windows)]
  {
    windows::install(token.clone())?;
  }
  Ok(ShutdownSignals { token })
}
