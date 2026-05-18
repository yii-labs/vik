//! Windows stub: foreground Ctrl-C only. Background detach + a real
//! SIGTERM equivalent lands later.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use super::SignalError;

pub fn send_sigterm(_pid: u32) -> std::io::Result<()> {
  Err(std::io::Error::other(
    "SIGTERM delivery is not supported on Windows yet",
  ))
}

pub fn pid_alive(_pid: u32) -> bool {
  false
}

pub fn install(shutdown: CancellationToken) -> Result<(), SignalError> {
  let forced = Arc::new(AtomicBool::new(false));
  let shutdown_clone = shutdown.clone();
  let forced_clone = Arc::clone(&forced);
  tokio::spawn(
    async move {
      loop {
        if tokio::signal::ctrl_c().await.is_err() {
          break;
        }
        if forced_clone.swap(true, Ordering::SeqCst) {
          tracing::error!(signal = "ctrl_c", "second shutdown signal; aborting");
          std::process::exit(130);
        }
        tracing::info!(
          signal = "ctrl_c",
          "shutdown signal received; requesting graceful shutdown",
        );
        shutdown_clone.cancel();
      }
    }
    .instrument(tracing::info_span!("daemon")),
  );
  Ok(())
}
