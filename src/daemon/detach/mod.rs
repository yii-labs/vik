//! Platform-split daemon detach.
//!
//! `vik run -d` puts the parent into the background so the launching
//! shell returns. On Unix that is the classic double-fork + setsid
//! dance plus a pipe handshake; on Windows the stub returns
//! `PlatformUnsupported` until issue 0011 lands.
//!
//! [`detach`] returns:
//! - `Ok(())` only in the **grandchild** — the surviving daemon.
//! - The original parent never returns: it `_exit(0)`s after reading
//!   the child's startup-ok byte. The shell observes a clean exit.
//! - `Err(DetachError)` only in the parent, when the child reports a
//!   startup failure through the handshake pipe.

use std::path::Path;

use thiserror::Error;

#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum DetachError {
  /// The `step` field names the syscall (`fork`, `setsid`, `dup2`,
  /// `open /dev/null`, …) so an operator can pinpoint the failure
  /// without a debugger.
  #[error("daemon detach failed at `{step}`: {source}")]
  Syscall {
    step: &'static str,
    #[source]
    source: std::io::Error,
  },

  /// Child wrote a diagnostic before `_exit`ing; surfaced verbatim.
  #[error("child process reported a startup failure: {message}")]
  ChildReportedFailure { message: String },

  #[error("daemon detach is not supported on this platform yet")]
  PlatformUnsupported,
}

/// `log_dir` is currently unused inside the grandchild — kept on the
/// signature so a future "writable-dir detection" or symlink-target
/// fallback does not break the caller.
pub fn detach(log_dir: &Path) -> Result<(), DetachError> {
  #[cfg(unix)]
  {
    unix::detach(log_dir)
  }
  #[cfg(windows)]
  {
    let _ = log_dir;
    windows::detach(log_dir)
  }
}
