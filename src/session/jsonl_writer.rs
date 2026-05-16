//! Append-only JSONL writer for session `AgentEvent`s.
//!
//! Wraps `tracing_appender::non_blocking` so writes never block the
//! session task: events are queued, a worker thread flushes to disk,
//! and `_guard` keeps the worker alive for the writer's lifetime.

use std::fs::File;
use std::io::{Result, Write};
use std::path::Path;

use tracing_appender::{
  non_blocking,
  non_blocking::{NonBlocking, WorkerGuard},
};

use crate::agent::AgentEvent;

pub(super) struct JsonlWriter {
  writer: NonBlocking,
  _guard: WorkerGuard,
}

impl JsonlWriter {
  pub(super) fn open(file: &Path) -> Result<Self> {
    // Append-only so a session that respawns (not a feature today, but
    // not impossible) cannot truncate prior history.
    let (writer, _guard) = non_blocking(File::options().append(true).create(true).open(file)?);

    Ok(Self { writer, _guard })
  }

  pub(super) fn write(&mut self, event: &AgentEvent) -> Result<usize> {
    let json = serde_json::to_vec(event)?;
    let len = json.len() + 1;

    self.writer.write_all(&json)?;
    self.writer.write_all(b"\n")?;

    Ok(len)
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn appends_events_as_json_lines_without_truncating_existing_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.jsonl");
    std::fs::write(&path, "{\"kind\":\"message\",\"text\":\"before\"}\n").expect("seed JSONL file");

    {
      let mut writer = JsonlWriter::open(&path).expect("writer opens");
      assert!(
        writer
          .write(&AgentEvent::SessionStarted {
            session_id: "session-1".into(),
            raw: None,
          })
          .expect("session-started event writes")
          > 0
      );
      assert!(
        writer
          .write(&AgentEvent::Completed { raw: None })
          .expect("completed event writes")
          > 0
      );
    }

    let lines = std::fs::read_to_string(&path)
      .expect("JSONL file reads")
      .lines()
      .map(|line| serde_json::from_str(line).expect("line is JSON"))
      .collect::<Vec<serde_json::Value>>();

    assert_eq!(
      lines,
      vec![
        json!({
          "kind": "message",
          "text": "before"
        }),
        json!({
          "kind": "session_started",
          "session_id": "session-1"
        }),
        json!({
          "kind": "completed"
        }),
      ]
    );
  }
}
