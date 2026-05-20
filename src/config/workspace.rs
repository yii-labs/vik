//! `workspace:` section of the Workflow Definition.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::WorkflowSchema;
use super::diagnose::*;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkspaceSchema {
  /// Workspace home before per-workflow namespacing. Missing or null
  /// uses `VIK_HOME` when set, otherwise the OS home `.vik` directory.
  /// Relative values are resolved against the workflow file directory
  /// (not cwd) at supervisor build time.
  #[serde(default)]
  pub root: Option<PathBuf>,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

impl Default for WorkspaceSchema {
  fn default() -> Self {
    Self {
      root: Some(".vik".into()),
      unknown_fields: Default::default(),
    }
  }
}

impl Diagnose for WorkspaceSchema {
  fn diagnose(&self, _: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    if let Some(root) = &self.root {
      diagnostics.error_if_empty_path("root", root);
    }
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

#[cfg(test)]
mod tests {
  use std::path::Path;

  use crate::config::WorkflowSchema;
  use crate::config::diagnose::Diagnose;
  use crate::config::diagnose::DiagnosticCode;

  use super::*;

  #[test]
  fn workspace_schema_defaults_to_repo_local_root() {
    let workspace = WorkspaceSchema::default();

    let diagnostics = workspace.diagnose(&WorkflowSchema::default());

    assert_eq!(workspace.root.as_deref(), Some(Path::new(".vik")));
    assert!(!diagnostics.has_errors());
    assert!(!diagnostics.has_warnings());
  }

  #[test]
  fn workspace_schema_accepts_null_root() {
    let workspace: WorkspaceSchema = serde_yaml::from_str("root: null").expect("workspace schema parses");

    let diagnostics = workspace.diagnose(&WorkflowSchema::default());

    assert_eq!(workspace.root, None);
    assert!(!diagnostics.has_errors());
  }

  #[test]
  fn workspace_schema_diagnoses_empty_root() {
    let workspace: WorkspaceSchema = serde_yaml::from_str("root: ''").expect("workspace schema parses");

    let diagnostics = workspace.diagnose(&WorkflowSchema::default());

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| diag.pointer == "root" && matches!(diag.code, DiagnosticCode::EmptyStr))
    );
  }
}
