//! Init-time pruning for the daily-rotated appenders.
//!
//! `tracing-appender::rolling::daily` rotates but does not prune. Vik
//! prunes only at [`super::init`] — the hot write path stays out of
//! `readdir` because logs fire many times a second and the per-write
//! cost would dominate. Daemon-restart cadence bounds disk usage well
//! enough.
//!
//! Only files matching one of the two known prefixes
//! (`super::INFO_LOG_PREFIX` / `super::ERROR_LOG_PREFIX`) followed by a
//! parseable `YYYY-MM-DD` are touched — we never want retention to
//! reach for operator files dropped into the same directory.

use std::path::Path;

use chrono::{Duration, NaiveDate, Utc};

use super::{ERROR_LOG_PREFIX, INFO_LOG_PREFIX};

/// Per-file removal failures are logged and skipped — startup must
/// not be blocked because one stale file refused to delete.
pub(crate) fn prune_old_logs(log_dir: &Path, retention_days: i64) -> std::io::Result<()> {
  let cutoff = Utc::now().date_naive() - Duration::days(retention_days);
  prune_with_cutoff(log_dir, cutoff)
}

/// Cutoff is split out so tests drive retention without depending on
/// the wall clock.
pub(crate) fn prune_with_cutoff(log_dir: &Path, cutoff: NaiveDate) -> std::io::Result<()> {
  let entries = match std::fs::read_dir(log_dir) {
    Ok(it) => it,
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
    Err(err) => return Err(err),
  };

  for entry in entries.flatten() {
    let path = entry.path();
    let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
      continue;
    };
    let Some(date) = parse_log_date(file_name) else {
      continue;
    };
    if date < cutoff
      && let Err(err) = std::fs::remove_file(&path)
    {
      tracing::info_span!("daemon").in_scope(|| {
        tracing::warn!(
          log_file = %path.display(),
          error = %err,
          "failed to remove stale log file",
        );
      });
    }
  }
  Ok(())
}

fn parse_log_date(file_name: &str) -> Option<NaiveDate> {
  for prefix in [INFO_LOG_PREFIX, ERROR_LOG_PREFIX] {
    let with_dot = format!("{prefix}.");
    if let Some(rest) = file_name.strip_prefix(&with_dot)
      && let Ok(date) = NaiveDate::parse_from_str(rest, "%Y-%m-%d")
    {
      return Some(date);
    }
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::fs::File;
  use std::io::Write;
  use std::path::PathBuf;

  fn tmp_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
      "vik-retention-{tag}-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("tmp dir");
    dir
  }

  fn touch(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    let mut f = File::create(&p).expect("touch file");
    writeln!(f, "dummy").expect("write dummy");
    p
  }

  #[test]
  fn deletes_old_files_keeps_fresh_ones() {
    let dir = tmp_dir("prune");
    // Comparison is `date < cutoff`, so dates equal to the cutoff
    // survive. The boundary case below pins this property — flipping
    // to `<=` would break that assertion.
    let cutoff = NaiveDate::from_ymd_opt(2026, 3, 10).expect("valid date");

    let old_info = touch(&dir, "vik.log.2026-03-01");
    let old_error = touch(&dir, "vik-error.log.2026-03-05");
    let fresh_info = touch(&dir, "vik.log.2026-03-15");
    let fresh_error = touch(&dir, "vik-error.log.2026-03-10");
    let alien = touch(&dir, "README.md");

    prune_with_cutoff(&dir, cutoff).expect("prune ok");

    assert!(!old_info.exists(), "old info file should be removed");
    assert!(!old_error.exists(), "old error file should be removed");
    assert!(fresh_info.exists(), "fresh info file should be kept");
    assert!(fresh_error.exists(), "file equal to the cutoff date must be kept");
    assert!(alien.exists(), "unrelated files must be left alone");

    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn missing_directory_is_ok() {
    let dir = std::env::temp_dir().join(format!(
      "vik-retention-missing-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
    ));
    // Do NOT create it.
    assert!(
      prune_with_cutoff(&dir, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()).is_ok(),
      "missing dir must be a no-op"
    );
  }
}
