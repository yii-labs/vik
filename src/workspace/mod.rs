//! Single source of truth for every Vik-owned path under the resolved
//! workflow workspace root.
//!
//! Callers anywhere else go through [`Workspace::logs_dir`],
//! [`Workspace::sessions_dir`], etc. Without this rule, a future
//! layout change would mean grepping the whole tree.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use thiserror::Error;

const ISSUES_DIR_NAME: &str = "issues";
const LOGS_DIR_NAME: &str = "logs";
const SESSIONS_DIR_NAME: &str = "sessions";
const SERVICE_DIR_NAME: &str = "service";
const SERVICE_STATE_FILE_NAME: &str = "state.json";

/// `OnceLock` per accessor so each derived path is allocated at most
/// once and the parameter-less getters can return `&Path`. `Sync`
/// matters because `Workspace` is shared via `Arc<Workflow>` across
/// orchestrator tasks.
#[derive(Debug, Default)]
pub struct Workspace {
  root: PathBuf,
  issues_dir: OnceLock<PathBuf>,
  logs_dir: OnceLock<PathBuf>,
  sessions_dir: OnceLock<PathBuf>,
  service_dir: OnceLock<PathBuf>,
  service_state_file: OnceLock<PathBuf>,
}

/// Errors surfaced while preparing `workspace.root`.
#[derive(Debug, Error)]
pub enum WorkspaceRootError {
  #[error("workspace.root {path} does not exist and its parent {parent} is also missing; create the parent first")]
  ParentMissing { path: PathBuf, parent: PathBuf },

  #[error("workspace.root {path} exists but is not a directory")]
  NotADirectory { path: PathBuf },

  #[error("workspace.root has no parent directory: {path}")]
  NoParent { path: PathBuf },

  #[error("failed to create workspace.root {path}: {source}")]
  Create {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("failed to stat workspace.root {path}: {source}")]
  Stat {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
}

impl Workspace {
  /// `root` is expected to be absolute already — the [`crate::workflow`]
  /// builder resolves `workspace.root` against the workflow file
  /// directory before constructing this. No re-validation here.
  pub fn new(root: PathBuf) -> Self {
    Self {
      root,
      ..Default::default()
    }
  }

  pub fn root(&self) -> &Path {
    &self.root
  }

  pub fn issues_dir(&self) -> &Path {
    self.issues_dir.get_or_init(|| self.root.join(ISSUES_DIR_NAME))
  }

  pub fn logs_dir(&self) -> &Path {
    self.logs_dir.get_or_init(|| self.root.join(LOGS_DIR_NAME))
  }

  pub fn sessions_dir(&self) -> &Path {
    self.sessions_dir.get_or_init(|| self.root.join(SESSIONS_DIR_NAME))
  }

  pub fn service_dir(&self) -> &Path {
    self.service_dir.get_or_init(|| self.root.join(SERVICE_DIR_NAME))
  }

  pub fn service_state_file(&self) -> &Path {
    self
      .service_state_file
      .get_or_init(|| self.service_dir().join(SERVICE_STATE_FILE_NAME))
  }

  pub fn issue_workdir(&self, issue_id: &str) -> PathBuf {
    self.issues_dir().join(issue_id)
  }

  pub fn issue_sessions_dir(&self, issue_id: &str) -> PathBuf {
    self.sessions_dir().join(issue_id)
  }

  /// Creates `root` if its parent already exists; refuses to create
  /// arbitrary-depth trees so a typo'd config does not silently
  /// produce a workspace far from the intended path.
  pub fn ensure_root(&self) -> Result<(), WorkspaceRootError> {
    let workspace_root = self.root();
    match std::fs::metadata(workspace_root) {
      Ok(meta) if meta.is_dir() => Ok(()),
      Ok(_) => Err(WorkspaceRootError::NotADirectory {
        path: workspace_root.to_path_buf(),
      }),
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
        let parent = workspace_root.parent().ok_or_else(|| WorkspaceRootError::NoParent {
          path: workspace_root.to_path_buf(),
        })?;

        // `Path::new(".vik").parent()` returns `Some("")` rather than
        // `None`, which would otherwise look like "parent missing."
        // Treat empty as "cwd," which by definition exists.
        let parent_exists = if parent.as_os_str().is_empty() {
          true
        } else {
          parent.is_dir()
        };

        if !parent_exists {
          return Err(WorkspaceRootError::ParentMissing {
            path: workspace_root.to_path_buf(),
            parent: parent.to_path_buf(),
          });
        }
        std::fs::create_dir(workspace_root).map_err(|source| WorkspaceRootError::Create {
          path: workspace_root.to_path_buf(),
          source,
        })
      },
      Err(err) => Err(WorkspaceRootError::Stat {
        path: workspace_root.to_path_buf(),
        source: err,
      }),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::fs::File;

  fn ws() -> Workspace {
    Workspace::new(PathBuf::from("/tmp/ws"))
  }

  #[test]
  fn root_accessor_returns_input() {
    let p = ws();
    assert_eq!(p.root(), Path::new("/tmp/ws"));
  }

  #[test]
  fn logs_and_sessions_dirs_live_under_root() {
    let p = ws();
    assert_eq!(p.logs_dir(), Path::new("/tmp/ws/logs"));
    assert_eq!(p.sessions_dir(), Path::new("/tmp/ws/sessions"));
  }

  #[test]
  fn issues_dir_lives_under_root_without_workflow_namespace_by_default() {
    let p = ws();
    assert_eq!(p.issues_dir(), Path::new("/tmp/ws/issues"));
  }

  #[test]
  fn service_dir_and_state_file_live_under_root() {
    let p = ws();
    assert_eq!(p.service_dir(), Path::new("/tmp/ws/service"));
    assert_eq!(p.service_state_file(), Path::new("/tmp/ws/service/state.json"));
  }

  #[test]
  fn issue_workdir_nests_under_issues_dir() {
    let p = ws();
    assert_eq!(p.issue_workdir("VIK-1"), PathBuf::from("/tmp/ws/issues/VIK-1"));
  }

  #[test]
  fn issue_sessions_dir_nests_issue_id_under_sessions() {
    let p = ws();
    assert_eq!(p.issue_sessions_dir("VIK-1"), PathBuf::from("/tmp/ws/sessions/VIK-1"));
  }

  #[test]
  fn memoized_accessors_return_stable_address_across_calls() {
    let p = ws();

    let a: *const Path = p.logs_dir();
    let b: *const Path = p.logs_dir();
    assert_eq!(a, b, "logs_dir() must hand back the cached PathBuf");

    let a: *const Path = p.sessions_dir();
    let b: *const Path = p.sessions_dir();
    assert_eq!(a, b, "sessions_dir() must hand back the cached PathBuf");

    let a: *const Path = p.service_dir();
    let b: *const Path = p.service_dir();
    assert_eq!(a, b, "service_dir() must hand back the cached PathBuf");

    let a: *const Path = p.service_state_file();
    let b: *const Path = p.service_state_file();
    assert_eq!(a, b, "service_state_file() must hand back the cached PathBuf");
  }

  #[test]
  fn creates_when_parent_exists() {
    let tempdir = tempfile::tempdir().unwrap();
    let target = tempdir.path().join("root");
    assert!(!target.exists());
    Workspace::new(target.clone()).ensure_root().expect("create ok");
    assert!(target.is_dir());
  }

  #[test]
  fn ensure_root_creates_only_workspace_root() {
    let tempdir = tempfile::tempdir().unwrap();
    let target = tempdir.path().join("root");

    Workspace::new(target.clone()).ensure_root().expect("create ok");

    assert!(target.is_dir());
    assert!(
      !target.join(".vik").exists(),
      "ensure_root must not create internal dirs"
    );
  }

  #[test]
  fn noop_when_already_a_directory() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let path = tempdir.path();
    Workspace::new(path.to_path_buf()).ensure_root().expect("noop ok");
    assert!(path.is_dir());
  }

  #[test]
  fn fails_when_parent_missing() {
    let anchor = tempfile::TempDir::new().unwrap();
    // Build a path whose parent does not exist.
    let target = anchor.path().join("missing-parent").join("root");
    let err = Workspace::new(target).ensure_root().expect_err("parent missing must fail");
    assert!(
      matches!(err, WorkspaceRootError::ParentMissing { .. }),
      "expected ParentMissing, got {err:?}"
    );
  }

  #[test]
  fn fails_when_target_is_a_file() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let as_file = tempdir.path().join("not-a-dir");
    File::create(&as_file).expect("touch");
    let err = Workspace::new(as_file).ensure_root().expect_err("file must fail");
    assert!(
      matches!(err, WorkspaceRootError::NotADirectory { .. }),
      "expected NotADirectory, got {err:?}"
    );
  }
}
