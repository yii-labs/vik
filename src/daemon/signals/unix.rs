//! SIGINT/SIGTERM/SIGHUP streams via `tokio::signal::unix`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use super::SignalError;

/// ESRCH ("pid already gone") collapses to success — there is nothing
/// for the caller to do about it, and the lifecycle layer already
/// handles the stale-file cleanup that would normally follow.
pub fn send_sigterm(pid: u32) -> std::io::Result<()> {
  use nix::sys::signal::{Signal, kill};
  use nix::unistd::Pid;
  match kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
    Ok(()) => Ok(()),
    Err(nix::errno::Errno::ESRCH) => Ok(()),
    Err(errno) => Err(std::io::Error::from_raw_os_error(errno as i32)),
  }
}

/// `kill(pid, 0)` — POSIX liveness check. EPERM means "exists but we
/// cannot signal it" which still counts as alive for our purposes.
pub fn pid_alive(pid: u32) -> bool {
  use nix::sys::signal::kill;
  use nix::unistd::Pid;
  match kill(Pid::from_raw(pid as i32), None) {
    Ok(()) => true,
    Err(nix::errno::Errno::EPERM) => true,
    Err(_) => false,
  }
}

pub fn install(shutdown: CancellationToken) -> Result<(), SignalError> {
  // Single shared latch across all three handlers so a Ctrl-C
  // followed by SIGTERM (or vice versa) still triggers the
  // second-signal abort.
  let forced = Arc::new(AtomicBool::new(false));

  install_stream(SignalKind::interrupt(), "SIGINT", &shutdown, &forced)?;
  install_stream(SignalKind::terminate(), "SIGTERM", &shutdown, &forced)?;

  // Default disposition for SIGHUP would terminate the process. We
  // explicitly install a tokio handler that logs and keeps running
  // — operators looking for a config-reload feature should see the
  // log and stop wondering why `kill -HUP` had no effect.
  let mut hup = signal(SignalKind::hangup()).map_err(SignalError::Install)?;
  tokio::spawn(
    async move {
      while hup.recv().await.is_some() {
        tracing::info!("SIGHUP received; ignoring (no reload configured)");
      }
    }
    .instrument(tracing::info_span!("daemon")),
  );

  Ok(())
}

fn install_stream(
  kind: SignalKind,
  label: &'static str,
  shutdown: &CancellationToken,
  forced: &Arc<AtomicBool>,
) -> Result<(), SignalError> {
  let mut stream = signal(kind).map_err(SignalError::Install)?;
  let shutdown = shutdown.clone();
  let forced = Arc::clone(forced);
  tokio::spawn(
    async move {
      while stream.recv().await.is_some() {
        if forced.swap(true, Ordering::SeqCst) {
          // Exit code 130 is the conventional "terminated by Ctrl-C"
          // status that most Unix shells expect.
          tracing::error!(signal = label, "second shutdown signal; aborting");
          std::process::exit(130);
        }
        tracing::info!(signal = label, "shutdown signal received; requesting graceful shutdown");
        shutdown.cancel();
      }
    }
    .instrument(tracing::info_span!("daemon")),
  );
  Ok(())
}
