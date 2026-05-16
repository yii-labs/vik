//! Runtime supervisor wrapping a parsed Workflow Definition schema.
//!
//! Where `WorkflowSchema` is parsed-only YAML, [`Workflow`] adds the
//! pieces that need a resolved file path: a [`Workspace`] anchored at
//! `workspace.root`, a [`HookRunner`] bound to this workflow, and loaded
//! stage prompt sources resolved relative to the workflow file directory.
//!
//! The split lets `vik doctor` validate raw YAML through
//! [`crate::config::WorkflowSchema`] without instantiating the workspace
//! or pulling in the agent registry.
#[cfg(test)]
mod builder;
pub mod loader;
mod prompts;

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use thiserror::Error;

use crate::config::diagnose::Diagnostics;
use crate::hooks::HookRunner;
use crate::workspace::Workspace;

pub use crate::config::*;
#[cfg(test)]
pub use builder::WorkflowBuilder;
pub use prompts::StagePromptSources;

#[derive(Debug)]
pub struct Workflow {
  workflow_dir: PathBuf,
  workflow_path: PathBuf,
  schema: WorkflowSchema,
  prompts: StagePromptSources,
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

  pub fn load(mut self) -> Result<Self, WorkflowError> {
    self.prompts = StagePromptSources::load(&self.workflow_dir, self.schema.issue.stages.iter())?;
    Ok(self)
  }

  pub fn get_stage_prompt(&self, stage_name: &str) -> Result<&str, WorkflowError> {
    self.prompts.template_for_stage(stage_name)
  }

  #[cfg(test)]
  pub(crate) fn prompt_stage_count(&self) -> usize {
    self.prompts.stage_count()
  }

  #[cfg(test)]
  pub(crate) fn prompt_file_count(&self) -> usize {
    self.prompts.file_count()
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

  #[error("prompt path `{0}` could not be resolved")]
  PromptPath(PathBuf),

  #[error("stage `{0}` prompt was not preloaded; call Workflow::load before launching sessions")]
  PromptNotLoaded(String),

  #[error("Failed to read prompt file {0}\n{1}")]
  PromptRead(PathBuf, #[source] std::io::Error),
}
