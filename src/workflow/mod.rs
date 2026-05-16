//! Runtime supervisor wrapping a parsed Workflow Definition schema.
//!
//! Where `WorkflowSchema` is parsed-only YAML, [`Workflow`] adds the
//! pieces that need a resolved file path: a [`Workspace`] anchored at
//! `workspace.root`, a [`HookRunner`] bound to this workflow, and helpers
//! that resolve other paths (prompt files, intake commands) relative to
//! the workflow file directory.
//!
//! The split lets `vik doctor` validate raw YAML through
//! [`crate::config::WorkflowSchema`] without instantiating the workspace
//! or pulling in the agent registry.
#[cfg(test)]
mod builder;
pub mod loader;

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use thiserror::Error;

use crate::config::diagnose::Diagnostics;
use crate::hooks::HookRunner;
use crate::utils;
use crate::workspace::Workspace;

pub use crate::config::*;
#[cfg(test)]
pub use builder::WorkflowBuilder;

#[derive(Debug)]
pub struct Workflow {
  workflow_dir: PathBuf,
  workflow_path: PathBuf,
  schema: WorkflowSchema,
  workspace: Workspace,
  hooks: HookRunner,
}

impl Workflow {
  pub fn schema(&self) -> &WorkflowSchema {
    &self.schema
  }

  pub fn workspace(&self) -> &Workspace {
    &self.workspace
  }

  pub(crate) fn workflow_path(&self) -> &Path {
    &self.workflow_path
  }

  pub fn agents(&self) -> &AgentProfilesSchema {
    &self.schema.agents
  }

  pub fn stages(&self) -> &IndexMap<String, issue::IssueStageSchema> {
    &self.schema.issue.stages
  }

  pub fn hooks(&self) -> &HookRunner {
    &self.hooks
  }

  /// Resolve a workflow-relative path string against the workflow file's
  /// directory. Used for prompt files, intake commands, anything declared
  /// in YAML as a relative path.
  pub fn resolve_path<P: AsRef<Path>>(&self, raw: P) -> Option<PathBuf> {
    utils::paths::resolve_from(&self.workflow_dir, raw)
  }
}

#[derive(Debug, Error)]
pub enum WorkflowError {
  #[error("Failed to resolve workflow path of {0}\n{1}")]
  PathResolution(PathBuf, #[source] std::io::Error),

  #[error("workflow file not found at {0}")]
  NotFound(PathBuf),

  #[error("workflow file {0} is a directory, expected a file")]
  IsDirectory(PathBuf),

  #[error("permission denied reading workflow file {0}")]
  PermissionDenied(PathBuf),

  #[error("Failed to read workflow file {0}\n{1}")]
  Read(PathBuf, #[source] std::io::Error),

  #[error("Failed to parse workflow YAML {0}\n{1}")]
  Yaml(PathBuf, #[source] serde_yaml::Error),

  #[error("invalid workflow config:\n{0}")]
  Diagnose(Diagnostics),

  #[error("workspace.root `{0}` could not be resolved")]
  WorkspaceRoot(PathBuf),
}
