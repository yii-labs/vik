use std::path::PathBuf;

use crate::config::IssueStageSchema;
use crate::config::WorkflowSchema;

use super::Workflow;

impl Workflow {
  pub fn builder() -> WorkflowBuilder {
    WorkflowBuilder::new()
  }
}

pub struct WorkflowBuilder {
  workflow_path: PathBuf,
  schema: WorkflowSchema,
}

#[cfg(test)]
impl WorkflowBuilder {
  pub fn new() -> Self {
    Self {
      workflow_path: "/virtual/path/to/workflow.yml".into(),
      schema: WorkflowSchema::default(),
    }
  }

  pub fn workflow_path(mut self, workflow_path: impl Into<PathBuf>) -> Self {
    self.workflow_path = workflow_path.into();
    self
  }

  pub fn max_issue_concurrency(mut self, max_issue_concurrency: u32) -> Self {
    self.schema.loop_.max_issue_concurrency = max_issue_concurrency;
    self
  }

  pub fn workspace_root(mut self, workspace_root: impl Into<PathBuf>) -> Self {
    self.schema.workspace.root = Some(workspace_root.into());
    self
  }

  pub fn without_workspace_root(mut self) -> Self {
    self.schema.workspace.root = None;
    self
  }

  pub fn pull_command(mut self, pull_command: impl Into<String>) -> Self {
    self.schema.issues.pull.command = pull_command.into();
    self
  }

  pub fn after_issue_workdir_create_hook(mut self, after_create: impl Into<String>) -> Self {
    self.schema.issue.hooks.after_create = Some(after_create.into());
    self
  }

  pub fn add_stage(
    mut self,
    name: impl Into<String>,
    state: impl Into<String>,
    prompt_file: impl Into<PathBuf>,
  ) -> Self {
    self
      .schema
      .issue
      .stages
      .push(IssueStageSchema::new(name, state).with_prompt_file(prompt_file));
    self
  }

  pub fn build(self) -> Workflow {
    Workflow::from_schema_unchecked(self.workflow_path, self.schema)
      .expect("Test workflow builder must build successfully")
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::utils;

  #[test]
  fn workflow_builder_starts_without_test_fixture_data() {
    let builder: crate::workflow::WorkflowBuilder = Workflow::builder();
    let workflow = builder.build();

    assert!(workflow.schema().agents.is_empty());
    assert!(workflow.schema().issues.pull.command.is_empty());
    assert!(workflow.schema().issue.stages.is_empty());
  }

  #[test]
  fn workflow_builder_applies_fluent_overrides() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.yml");

    let workflow = Workflow::builder()
      .max_issue_concurrency(10)
      .workspace_root(temp.path())
      .workflow_path(workflow_path.clone())
      .pull_command("printf '%s' '[]'")
      .after_issue_workdir_create_hook("echo created")
      .add_stage("implement", "todo", "./implement.md")
      .build();

    assert_eq!(workflow.schema().loop_.max_issue_concurrency, 10);
    assert_eq!(workflow.schema().workspace.root.as_deref(), Some(temp.path()));
    assert_eq!(workflow.schema().issues.pull.command, "printf '%s' '[]'");
    assert_eq!(
      workflow.schema().issue.hooks.after_create.as_deref(),
      Some("echo created")
    );
    assert_eq!(
      workflow
        .schema()
        .issue
        .stages
        .iter()
        .map(|stage| stage.name.as_str())
        .collect::<Vec<_>>(),
      ["implement"]
    );
    assert_eq!(
      workflow.workspace().root(),
      temp
        .path()
        .join("workflows")
        .join(workflow_path.to_string_lossy().replace('/', "-"))
    );
  }

  #[test]
  fn workflow_root_resolution() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.yml");
    let workflow = Workflow::builder()
      .workspace_root(".vik")
      .workflow_path(workflow_path.clone())
      .build();

    assert_eq!(
      workflow.workspace().root(),
      temp
        .path()
        .join(".vik")
        .join("workflows")
        .join(workflow_path.to_string_lossy().replace('/', "-"))
    );
  }

  #[test]
  fn workflow_without_workspace_root_defaults_to_home_vik_and_workflow_namespace() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.yml");
    let workflow = Workflow::builder()
      .workflow_path(workflow_path.clone())
      .without_workspace_root()
      .build();

    assert_eq!(
      workflow.workspace().root(),
      utils::paths::default_home()
        .join("workflows")
        .join(workflow_path.to_string_lossy().replace('/', "-"))
    );
  }
}
