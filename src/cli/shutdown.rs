//! Graceful-shutdown helper for `vik run`.
//!
//! Lives in `cli/` rather than `daemon/` because `daemon` should not
//! depend on the orchestrator or HTTP server. This is the single seam
//! that races the two halves against the shutdown-token deadline.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::logging::Phase;

/// Wall-clock budget for the graceful half of shutdown.
pub const GRACE: Duration = Duration::from_secs(30);

/// Run `join_future` until either it completes or `shutdown` trips,
/// then race it against [`GRACE`]. Never trips the token itself —
/// callers wanting to abort programmatically use `shutdown.cancel()`
/// on the token they passed in. State-file cleanup and dropping the
/// logging guard belong to the caller.
pub async fn graceful<F>(shutdown: CancellationToken, join_future: F) -> F::Output
where
  F: std::future::Future,
{
  // `Box::pin` so we can re-await the same future across the two
  // select arms below without moving it.
  let mut pinned: std::pin::Pin<Box<F>> = Box::pin(join_future);

  tokio::select! {
      // `biased` ensures a future that completes naturally on the
      // same tick the token trips still returns its result rather
      // than getting swallowed by the cancel arm.
      biased;
      output = &mut pinned => {
          return output;
      }
      _ = shutdown.cancelled() => {
          tracing::info!(
              phase = %Phase::Daemon,
              grace_ms = GRACE.as_millis() as u64,
              "shutdown token tripped; entering graceful shutdown",
          );
      }
  }

  let result = tokio::time::timeout(GRACE, pinned.as_mut()).await;
  match result {
    Ok(val) => val,
    // Past the deadline we still wait — there is no way to abort the
    // future safely from here, but the warn tells the operator the
    // graceful budget was exceeded.
    Err(_timeout) => {
      tracing::warn!(
          phase = %Phase::Daemon,
          grace_ms = GRACE.as_millis() as u64,
          "graceful shutdown deadline expired; waiting for runtime to finish",
      );
      pinned.await
    },
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn graceful_returns_ready_future_even_if_shutdown_already_cancelled() {
    let shutdown = CancellationToken::new();
    shutdown.cancel();

    let output = graceful(shutdown, async { "finished" }).await;

    assert_eq!(output, "finished");
  }
}
