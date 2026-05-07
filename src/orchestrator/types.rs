//! Runtime value types used by the orchestrator.
//!
//! [`IssueStage`] pairs an intake `Issue` with one matched workflow stage
//! without modifying either. The workflow schema stores stages in an
//! `IndexMap<String, ...>`; [`Stage`] carries the map key alongside the
//! schema so the runtime never has to look the name back up by reverse
//! lookup. [`IssueStageKey`] is the `(issue_id, stage_name)` pair used
//! as the primary key in [`super::running::RunningMap`].

use crate::config::IssueStage as StageSchema;
use crate::context::Issue;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct IssueStageKey {
  pub(super) issue_id: String,
  pub(super) stage_name: String,
}

impl IssueStageKey {
  pub(super) fn new(issue_id: impl Into<String>, stage_name: impl Into<String>) -> Self {
    Self {
      issue_id: issue_id.into(),
      stage_name: stage_name.into(),
    }
  }
}

#[derive(Debug, Clone)]
pub(super) struct Stage {
  name: String,
  schema: StageSchema,
}

impl Stage {
  fn new(name: String, schema: StageSchema) -> Self {
    Self { name, schema }
  }

  pub(super) fn name(&self) -> &str {
    &self.name
  }

  pub(super) fn schema(&self) -> &StageSchema {
    &self.schema
  }
}

/// Multiple values may share an `Issue` when several stages match the
/// same `issue.state`.
#[derive(Debug, Clone)]
pub(super) struct IssueStage {
  issue: Issue,
  stage: Stage,
}

impl IssueStage {
  pub(super) fn new(issue: Issue, stage_name: String, stage_schema: StageSchema) -> Self {
    Self {
      issue,
      stage: Stage::new(stage_name, stage_schema),
    }
  }

  pub(super) fn key(&self) -> IssueStageKey {
    IssueStageKey::new(self.issue.id.clone(), self.stage.name.clone())
  }

  pub(super) fn issue(&self) -> &Issue {
    &self.issue
  }

  pub(super) fn stage(&self) -> &Stage {
    &self.stage
  }
}
