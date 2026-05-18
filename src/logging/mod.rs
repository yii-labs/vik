//! Global tracing subscriber wiring.
//!
//! Three layers: stdout (foreground only — detached runs would just
//! burn CPU serializing into a `/dev/null`-redirected fd), one daily-
//! rotated INFO file, one daily-rotated ERROR file. Retention is
//! 7 days, enforced eagerly at [`init`] instead of on each write — log
//! emission is hot-path and scanning the directory there would dominate
//! the writer budget for no operational gain.
pub(crate) mod retention;

#[cfg(test)]
pub(crate) mod tests;

use std::path::Path;

use thiserror::Error;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

pub(crate) const INFO_LOG_PREFIX: &str = "vik.log";
pub(crate) const ERROR_LOG_PREFIX: &str = "vik-error.log";
pub(crate) const RETENTION_DAYS: i64 = 7;

/// Owns the background flush threads. Drop at process exit; the
/// `must_use` lint catches forgotten variables that would otherwise
/// cut off log output before flushes complete.
#[must_use = "dropping the guard flushes and closes the file appenders; keep it alive for the process lifetime"]
pub struct LoggingGuard {
  _info_guard: WorkerGuard,
  _error_guard: WorkerGuard,
}

/// Errors surfaced by [`init`].
#[derive(Debug, Error)]
pub enum LoggingError {
  #[error("failed to create log directory {path}: {source}")]
  CreateLogDir {
    path: std::path::PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("log directory {path} exists but is not a directory")]
  LogDirNotADirectory { path: std::path::PathBuf },

  #[error("failed to install global tracing subscriber (already set?): {0}")]
  SetGlobal(String),
}

/// Calling twice errors via [`LoggingError::SetGlobal`] — tests that
/// need a subscriber should build one with the helpers in
/// [`crate::logging::layers`] inside a `tracing::subscriber::with_default`
/// scope, never `init` twice.
pub fn init(log_dir: &Path, enable_stdout: bool) -> Result<LoggingGuard, LoggingError> {
  ensure_log_dir(log_dir)?;

  // `tracing-appender::rolling::daily` rotates but does not prune;
  // `retention.rs` handles that, eagerly on init only — see its
  // module doc for why the hot write path stays untouched.
  let info_appender = rolling::daily(log_dir, INFO_LOG_PREFIX);
  let (info_writer, info_guard) = tracing_appender::non_blocking(info_appender);

  let error_appender = rolling::daily(log_dir, ERROR_LOG_PREFIX);
  let (error_writer, error_guard) = tracing_appender::non_blocking(error_appender);

  let default_filter_builder = || {
    EnvFilter::builder()
      .with_default_directive("info".parse().unwrap())
      .from_env_lossy()
  };

  // Skip the stdout layer entirely (rather than `with_writer(/dev/null)`)
  // when disabled — otherwise we pay full serialization cost per event
  // for a layer nothing reads.
  let stdout_layer = if enable_stdout {
    Some(
      tracing_subscriber::fmt::layer()
        .compact()
        .with_writer(std::io::stdout)
        .with_filter(default_filter_builder()),
    )
  } else {
    None
  };

  let info_file_layer = tracing_subscriber::fmt::layer()
    .json()
    .with_current_span(true)
    .with_span_list(true)
    .flatten_event(true)
    .with_ansi(false)
    .with_writer(info_writer)
    .with_filter(default_filter_builder());

  let error_file_layer = tracing_subscriber::fmt::layer()
    .json()
    .with_current_span(true)
    .with_span_list(true)
    .flatten_event(true)
    .with_ansi(false)
    .with_writer(error_writer)
    .with_filter(EnvFilter::new("error"));

  let registry = tracing_subscriber::registry()
    .with(stdout_layer)
    .with(info_file_layer)
    .with(error_file_layer);

  registry.try_init().map_err(|err| LoggingError::SetGlobal(err.to_string()))?;

  // Prune after the subscriber is live so any failure goes through
  // the same structured stream operators are already watching.
  // Retention failures must not block startup — disk pressure is
  // operator-visible through the warning + filesystem.
  if let Err(err) = retention::prune_old_logs(log_dir, RETENTION_DAYS) {
    tracing::warn!(
        log_dir = %log_dir.display(),
        error = %err,
        "log retention scan failed; leaving old files in place",
    );
  }

  Ok(LoggingGuard {
    _info_guard: info_guard,
    _error_guard: error_guard,
  })
}

fn ensure_log_dir(log_dir: &Path) -> Result<(), LoggingError> {
  match std::fs::metadata(log_dir) {
    Ok(meta) if meta.is_dir() => Ok(()),
    Ok(_) => Err(LoggingError::LogDirNotADirectory {
      path: log_dir.to_path_buf(),
    }),
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
      std::fs::create_dir_all(log_dir).map_err(|source| LoggingError::CreateLogDir {
        path: log_dir.to_path_buf(),
        source,
      })
    },
    Err(err) => Err(LoggingError::CreateLogDir {
      path: log_dir.to_path_buf(),
      source: err,
    }),
  }
}

#[cfg(test)]
mod value_tests {
  use super::{ERROR_LOG_PREFIX, INFO_LOG_PREFIX, RETENTION_DAYS};

  #[test]
  fn logging_module_values_match_operational_contract() {
    assert_eq!(INFO_LOG_PREFIX, "vik.log");
    assert_eq!(ERROR_LOG_PREFIX, "vik-error.log");
    assert_eq!(RETENTION_DAYS, 7);
  }
}
