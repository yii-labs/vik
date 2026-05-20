//! Daemon state file persistence at `<workflow-workspace-root>/service/state.json`.
//!
//! Records just enough for lifecycle verbs (`status`/`stop`/`restart`/
//! `uninstall`) to manage the daemon without re-parsing the workflow:
//! pid, port, log/sessions paths, and the cwd the daemon was launched
//! from (so workflow-relative paths resolve identically on restart).
//!
//! Writes go through a sibling tempfile + atomic `rename` so a crash
//! during write cannot leave a half-JSON file. Reads tolerate absence
//! (`Ok(None)`) so callers can distinguish "no daemon" from "broken
//! file."

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::workflow::Workflow;

#[derive(Debug, Clone)]
pub struct StateManager {
  path: PathBuf,
}

impl StateManager {
  pub fn new(path: impl Into<PathBuf>) -> Self {
    Self { path: path.into() }
  }

  pub fn path(&self) -> &Path {
    &self.path
  }

  pub fn read(&self) -> Result<Option<State>, StateError> {
    State::try_read(&self.path)
  }

  pub fn write(&self, state: &State) -> Result<(), StateError> {
    state.write(&self.path)
  }

  pub fn remove(&self) -> Result<(), StateError> {
    State::remove(&self.path)
  }

  /// `command` is captured verbatim so an operator looking at a stale
  /// state file can tell which invocation produced it.
  pub fn write_runtime_state(
    &self,
    workflow: &Workflow,
    port: u16,
    bind_address: String,
    command: String,
  ) -> Result<(), StateError> {
    let state = State {
      workflow_path: workflow.workflow_path().to_path_buf(),
      cwd: std::env::current_dir().map_err(StateError::CurrentDir)?,
      pid: std::process::id(),
      port,
      bind_address,
      started_at: Utc::now(),
      log_dir: workflow.workspace().logs_dir().to_path_buf(),
      sessions_dir: workflow.workspace().sessions_dir().to_path_buf(),
      command,
    };
    self.write(&state)?;
    tracing::info_span!("daemon").in_scope(|| {
      tracing::info!(
        state_file = %self.path.display(),
        pid = state.pid,
        port = state.port as u64,
        "daemon state file written",
      );
    });
    Ok(())
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
  pub workflow_path: PathBuf,
  /// Captured so `vik restart` resolves workflow-relative paths the
  /// way the original invocation did, regardless of where restart is
  /// run from.
  pub cwd: PathBuf,
  pub pid: u32,
  /// Normal runs record the actual bound HTTP port. `0` is reserved
  /// for older or manually-authored state files that lack HTTP info.
  pub port: u16,
  /// String form so IPv4 and IPv6 round-trip unchanged.
  pub bind_address: String,
  pub started_at: DateTime<Utc>,
  pub log_dir: PathBuf,
  pub sessions_dir: PathBuf,
  /// Raw command line for operator debugging — no parser ever consumes
  /// this.
  pub command: String,
}

#[derive(Debug, Error)]
pub enum StateError {
  #[error("failed to read daemon state file {path}: {source}")]
  Read {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  /// Often a sign of an old build's state file. The operator can
  /// `vik uninstall` and retry.
  #[error("failed to parse daemon state file {path}: {source}")]
  Parse {
    path: PathBuf,
    #[source]
    source: serde_json::Error,
  },

  #[error("failed to write daemon state file {path}: {source}")]
  Write {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("failed to serialize daemon state: {0}")]
  Serialize(#[source] serde_json::Error),

  #[error("failed to read current working directory: {0}")]
  CurrentDir(#[source] std::io::Error),

  #[error("failed to remove daemon state file {path}: {source}")]
  Remove {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
}

impl State {
  /// `Ok(None)` is used for "file missing" so callers can distinguish
  /// it from "file broken" without inspecting the error variant.
  pub fn try_read(path: &Path) -> Result<Option<Self>, StateError> {
    match std::fs::read(path) {
      Ok(bytes) => {
        let state: State = serde_json::from_slice(&bytes).map_err(|source| StateError::Parse {
          path: path.to_path_buf(),
          source,
        })?;
        Ok(Some(state))
      },
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
      Err(source) => Err(StateError::Read {
        path: path.to_path_buf(),
        source,
      }),
    }
  }

  pub fn write(&self, path: &Path) -> Result<(), StateError> {
    if let Some(parent) = path.parent() {
      std::fs::create_dir_all(parent).map_err(|source| StateError::Write {
        path: parent.to_path_buf(),
        source,
      })?;
    }

    let body = serde_json::to_vec_pretty(self).map_err(StateError::Serialize)?;

    let mut tmp = path.to_path_buf();
    // Sibling filename in the same directory so `rename` is atomic.
    // Suffix with pid so two concurrent writers cannot collide on the
    // tempfile name.
    let mut name = match path.file_name() {
      Some(n) => n.to_os_string(),
      None => {
        return Err(StateError::Write {
          path: path.to_path_buf(),
          source: std::io::Error::other("state file path has no filename component"),
        });
      },
    };
    name.push(format!(".tmp.{}", std::process::id()));
    tmp.set_file_name(name);

    let mut file = std::fs::OpenOptions::new()
      .create(true)
      .truncate(true)
      .write(true)
      .open(&tmp)
      .map_err(|source| StateError::Write {
        path: tmp.clone(),
        source,
      })?;
    file.write_all(&body).map_err(|source| StateError::Write {
      path: tmp.clone(),
      source,
    })?;
    file.sync_all().map_err(|source| StateError::Write {
      path: tmp.clone(),
      source,
    })?;
    drop(file);

    std::fs::rename(&tmp, path).map_err(|source| StateError::Write {
      path: path.to_path_buf(),
      source,
    })?;

    Ok(())
  }

  /// Missing-file is not an error — graceful shutdown removes the file
  /// and a second cleanup (e.g. signal handler racing `vik stop`) must
  /// still succeed.
  pub fn remove(path: &Path) -> Result<(), StateError> {
    match std::fs::remove_file(path) {
      Ok(()) => Ok(()),
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
      Err(source) => Err(StateError::Remove {
        path: path.to_path_buf(),
        source,
      }),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::logging::tests::{capture_events, captured_event};
  use crate::workflow::Workflow;
  use tempfile::TempDir;

  fn sample(workflow: &Path, cwd: &Path) -> State {
    State {
      workflow_path: workflow.to_path_buf(),
      cwd: cwd.to_path_buf(),
      pid: 1234,
      port: 3000,
      bind_address: "127.0.0.1".into(),
      started_at: "2026-05-09T10:00:00Z".parse().unwrap(),
      log_dir: cwd.join("logs"),
      sessions_dir: cwd.join("sessions"),
      command: "vik run -d workflow.yml".into(),
    }
  }

  #[test]
  fn roundtrip_preserves_fields() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    let state = sample(&dir.path().join("workflow.yml"), dir.path());
    state.write(&path).expect("write ok");
    let back = State::try_read(&path).expect("read ok").expect("present");
    assert_eq!(back, state);
  }

  #[test]
  fn try_read_missing_returns_none() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    let res = State::try_read(&path).expect("missing is Ok(None)");
    assert!(res.is_none());
  }

  #[test]
  fn write_creates_parent_directory() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("nested/a/b/state.json");
    let state = sample(&dir.path().join("workflow.yml"), dir.path());
    state.write(&path).expect("write ok");
    assert!(path.exists());
  }

  #[test]
  fn remove_missing_is_ok() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("never-created.json");
    State::remove(&path).expect("remove missing ok");
  }

  #[test]
  fn remove_existing_deletes_file() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    let state = sample(&dir.path().join("workflow.yml"), dir.path());
    state.write(&path).expect("write");
    assert!(path.exists());
    State::remove(&path).expect("remove");
    assert!(!path.exists());
  }

  #[test]
  fn state_manager_writes_runtime_state_with_daemon_span() {
    let (events, _capture) = capture_events();

    let dir = TempDir::new().expect("tmpdir");
    let workflow = Workflow::builder()
      .workflow_path(dir.path().join("workflow.yml"))
      .workspace_root(dir.path())
      .build();
    let manager = StateManager::new(dir.path().join("service/state.json"));

    manager
      .write_runtime_state(&workflow, 3456, "127.0.0.1".into(), "vik run workflow.yml".into())
      .expect("write runtime state");

    let state = manager.read().expect("state reads").expect("state exists");
    assert_eq!(state.workflow_path, dir.path().join("workflow.yml"));
    assert_eq!(state.port, 3456);
    assert_eq!(state.bind_address, "127.0.0.1");
    assert_eq!(state.command, "vik run workflow.yml");

    let events = events.lock().expect("events mutex");
    let event = captured_event(&events, "daemon state file written");
    assert_eq!(event["spans"][0]["name"], "daemon");
    assert!(event.get("phase").is_none());
  }

  #[test]
  fn try_read_errors_on_malformed_json() {
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().join("state.json");
    std::fs::write(&path, b"{not json").expect("seed");
    let err = State::try_read(&path).expect_err("bad json must fail");
    assert!(matches!(err, StateError::Parse { .. }));
  }
}
