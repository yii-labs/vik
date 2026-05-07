//! Shared primitive for spawning subprocesses with timeout + cancel.

mod command_ext;

use thiserror::Error;

pub use command_ext::*;

#[derive(Debug, Error)]
pub enum CommandExecError {
  #[error("failed to spawn subprocess: {0}")]
  Spawn(#[source] std::io::Error),

  #[error("subprocess timed out after {duration_ms} ms")]
  Timeout { duration_ms: u64 },

  /// Killed by the OS (OOM, signal from outside Vik). Distinct from
  /// `Cancelled` so dispatch logs can tell operator-driven shutdown
  /// apart from system pressure.
  #[allow(dead_code)]
  #[error("subprocess got killed after {duration_ms} ms")]
  Killed { duration_ms: u64 },

  /// Killed by Vik itself — shutdown token tripped or `Child::cancel`
  /// was called. Treated as cooperative termination by callers.
  #[error("subprocess got cancelled after {duration_ms} ms")]
  Cancelled { duration_ms: u64 },
}
