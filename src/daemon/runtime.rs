//! Daemon runtime driver.
//!
//! The CLI prepares process concerns, then hands daemon runtime this pair:
//! orchestrator future plus optional HTTP server future. This module does not
//! import either subsystem, so ownership stays here without adding cycles.

use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

/// Wall-clock budget for the graceful half of shutdown.
pub const GRACE: Duration = Duration::from_secs(30);

pub async fn drive<O, S>(shutdown: CancellationToken, orchestrator: O, server: Option<S>) -> Result<()>
where
  O: Future<Output = Result<()>>,
  S: Future<Output = Result<()>>,
{
  match server {
    Some(server) => graceful(shutdown.clone(), drive_pair(shutdown, orchestrator, server)).await,
    None => graceful(shutdown, orchestrator).await,
  }
}

async fn drive_pair<O, S>(shutdown: CancellationToken, orchestrator: O, server: S) -> Result<()>
where
  O: Future<Output = Result<()>>,
  S: Future<Output = Result<()>>,
{
  tokio::pin!(orchestrator);
  tokio::pin!(server);

  let mut orchestrator_result = None;
  let mut server_result = None;

  loop {
    tokio::select! {
      result = &mut orchestrator, if orchestrator_result.is_none() => {
        shutdown.cancel();
        orchestrator_result = Some(result);
      },
      result = &mut server, if server_result.is_none() => {
        shutdown.cancel();
        server_result = Some(result);
      },
    }

    if orchestrator_result.is_some() && server_result.is_some() {
      break;
    }
  }

  orchestrator_result.expect("orchestrator future completed")?;
  server_result.expect("server future completed")?;
  Ok(())
}

/// Run `join_future` until either it completes or `shutdown` trips, then race
/// it against [`GRACE`]. State-file cleanup belongs to caller.
async fn graceful<F>(shutdown: CancellationToken, join_future: F) -> F::Output
where
  F: Future,
{
  let mut pinned: std::pin::Pin<Box<F>> = Box::pin(join_future);

  tokio::select! {
      biased;
      output = &mut pinned => {
          return output;
      }
      _ = shutdown.cancelled() => {
        tracing::info_span!("daemon").in_scope(|| {
          tracing::info!(
              grace_ms = GRACE.as_millis() as u64,
              "shutdown token tripped; entering graceful shutdown",
          );
        });
      }
  }

  let result = tokio::time::timeout(GRACE, pinned.as_mut()).await;
  match result {
    Ok(val) => val,
    Err(_timeout) => {
      tracing::info_span!("daemon").in_scope(|| {
        tracing::warn!(
          grace_ms = GRACE.as_millis() as u64,
          "graceful shutdown deadline expired; waiting for runtime to finish",
        );
      });
      pinned.await
    },
  }
}

#[cfg(test)]
mod tests {
  use anyhow::anyhow;

  use super::*;

  #[tokio::test]
  async fn drive_without_server_returns_orchestrator_result() {
    let shutdown = CancellationToken::new();

    let result = drive(shutdown, async { Ok(()) }, None::<std::future::Ready<Result<()>>>).await;

    assert!(result.is_ok());
  }

  #[tokio::test]
  async fn drive_with_server_cancels_server_when_orchestrator_finishes() {
    let shutdown = CancellationToken::new();
    let server_shutdown = shutdown.clone();

    let result = drive(
      shutdown,
      async { Ok(()) },
      Some(async move {
        server_shutdown.cancelled().await;
        Ok(())
      }),
    )
    .await;

    assert!(result.is_ok());
  }

  #[tokio::test]
  async fn drive_with_server_error_cancels_orchestrator_and_returns_error() {
    let shutdown = CancellationToken::new();
    let orchestrator_shutdown = shutdown.clone();

    let result = drive(
      shutdown,
      async move {
        orchestrator_shutdown.cancelled().await;
        Ok(())
      },
      Some(async { Err(anyhow!("server failed")) }),
    )
    .await;

    assert!(result.expect_err("server error").to_string().contains("server failed"));
  }
}
