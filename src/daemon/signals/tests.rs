#[cfg(unix)]
#[cfg(test)]
mod unix_tests {
  use std::time::Duration;

  use super::super::install_shutdown_handler;

  /// SIGTERM rather than SIGINT because some test runners intercept
  /// SIGINT for their own bookkeeping.
  #[tokio::test]
  async fn sigterm_trips_shutdown_token() {
    let signals = install_shutdown_handler().expect("install handler");
    let token = signals.token();

    let me = std::process::id();
    unsafe {
      // Safety: SIGTERM is a valid signal number; the kernel rejects
      // bad arguments by returning -1, which we ignore here.
      libc::kill(me as libc::pid_t, libc::SIGTERM);
    }

    let waited = tokio::time::timeout(Duration::from_secs(2), token.cancelled()).await;
    assert!(waited.is_ok(), "shutdown token did not trip within 2s");
  }
}
