//! `workspace:` section of the Workflow Definition.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::WorkflowSchema;
use super::diagnose::*;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkspaceSchema {
  /// Workspace home before per-workflow namespacing. Missing or null
  /// uses `VIK_HOME` when set, otherwise the OS home directory.
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
