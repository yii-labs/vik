use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::config::IssueStage as StageSchema;
use crate::hooks::HookError;
use crate::template::StageContext;
use crate::workflow::Workflow;

use super::Issue;

#[derive(Debug, Error)]
pub enum IssueRunError {
  #[error("failed to create issue workspace `{path}`: {source}")]
  CreateWorkspace {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
  #[error(transparent)]
  Hook(#[from] HookError),
}

#[derive(Debug)]
pub struct IssueRun {
  workflow: Arc<Workflow>,
  issue: Issue,
  issue_workdir: PathBuf,
}

impl IssueRun {
  pub fn new(workflow: Arc<Workflow>, issue: Issue) -> Self {
    let issue_workdir = workflow.workspace().issue_workdir(&issue.id);
    Self {
      workflow,
      issue,
      issue_workdir,
    }
  }

  pub fn workflow(&self) -> &Workflow {
    self.workflow.as_ref()
  }

  pub fn issue(&self) -> &Issue {
    &self.issue
  }

  pub fn issue_workdir(&self) -> &Path {
    &self.issue_workdir
  }

  pub fn matching_stages(issue_run: Arc<Self>) -> Vec<IssueStage> {
    issue_run
      .workflow()
      .stages()
      .iter()
      .filter(|(_, stage)| stage.when.state == issue_run.issue.state)
      .map(|(name, stage)| IssueStage::new(Arc::clone(&issue_run), name.clone(), stage.clone()))
      .collect()
  }

  pub async fn prepare(&self) -> Result<(), IssueRunError> {
    match tokio::fs::metadata(&self.issue_workdir).await {
      Ok(metadata) if metadata.is_dir() => {
        tracing::debug!(path = %self.issue_workdir.display(), "issue workspace already exists; skipping creation");
        return Ok(());
      },
      Ok(_) => {
        return Err(IssueRunError::CreateWorkspace {
          path: self.issue_workdir.clone(),
          source: std::io::Error::other("path exists but is not a directory"),
        });
      },
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
      Err(e) => {
        return Err(IssueRunError::CreateWorkspace {
          path: self.issue_workdir.clone(),
          source: e,
        });
      },
    };

    tokio::fs::create_dir_all(&self.issue_workdir)
      .await
      .map_err(|source| IssueRunError::CreateWorkspace {
        path: self.issue_workdir.clone(),
        source,
      })?;

    self
      .workflow()
      .hooks()
      .run_after_create(&self.workflow().schema().issue.hooks, &self.issue, &self.issue_workdir)
      .await?;

    Ok(())
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IssueStageKey {
  pub issue_id: String,
  pub stage_name: String,
}

impl IssueStageKey {
  pub fn new(issue_id: impl Into<String>, stage_name: impl Into<String>) -> Self {
    Self {
      issue_id: issue_id.into(),
      stage_name: stage_name.into(),
    }
  }
}

#[derive(Debug, Clone)]
pub struct IssueStage {
  issue_run: Arc<IssueRun>,
  stage_name: String,
  stage: StageSchema,
}

impl IssueStage {
  pub fn new(issue_run: Arc<IssueRun>, stage_name: String, stage_schema: StageSchema) -> Self {
    Self {
      issue_run,
      stage_name,
      stage: stage_schema,
    }
  }

  pub fn issue_run(&self) -> &IssueRun {
    self.issue_run.as_ref()
  }

  pub fn workflow(&self) -> &Workflow {
    self.issue_run.workflow()
  }

  pub fn issue(&self) -> &Issue {
    self.issue_run.issue()
  }

  pub fn issue_workdir(&self) -> &Path {
    self.issue_run.issue_workdir()
  }

  pub fn stage_name(&self) -> &str {
    &self.stage_name
  }

  pub fn stage(&self) -> &StageSchema {
    &self.stage
  }

  pub fn key(&self) -> IssueStageKey {
    IssueStageKey::new(self.issue().id.clone(), self.stage_name.clone())
  }

  pub fn template_context(&self) -> StageContext<'_> {
    StageContext {
      issue: self.issue(),
      stage_name: self.stage_name(),
      agent_profile: &self.stage.agent,
      stage_state: &self.stage.when.state,
      issue_workdir: self.issue_workdir(),
      workspace_root: self.workflow().workspace().root(),
    }
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use super::*;
  use crate::workflow::Workflow;
  use crate::workflow::loader::WorkflowSchemaLoader;

  #[tokio::test]
  async fn prepare_skips_after_create_when_issue_workdir_exists() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let workflow_path = temp.path().join("workflow.yml");
    let workflow = workflow_fixture(&root, workflow_path, "echo should-not-run >> after_create.log");
    let issue_run = IssueRun::new(Arc::new(workflow), issue("ABC-1", "todo"));

    std::fs::create_dir_all(issue_run.issue_workdir()).expect("issue workdir exists");

    issue_run.prepare().await.expect("prepare succeeds");

    assert!(
      !issue_run.issue_workdir().join("after_create.log").exists(),
      "existing issue workdir skips after_create"
    );
  }

  fn workflow_fixture(root: &std::path::Path, workflow_path: std::path::PathBuf, after_create: &str) -> Workflow {
    let root_yaml = root.to_string_lossy();
    let loaded = WorkflowSchemaLoader
      .load_from_str(&workflow_yaml(root_yaml.as_ref(), after_create), Some(workflow_path))
      .expect("workflow schema parses");

    Workflow::try_from(loaded).expect("load workflow")
  }

  fn workflow_yaml(root_yaml: &str, after_create: &str) -> String {
    format!(
      r#"
loop:
  max_issue_concurrency: 1
  wait_ms: 10
workspace:
  root: '{root_yaml}'
agents:
  codex:
    runtime: codex
    model: gpt-5.5
issues:
  pull:
    command: ./issues-json
    idle_sec: 1
issue:
  hooks:
    after_create: {after_create}
  stages:
    plan:
      when:
        state: todo
      agent: codex
      prompt_file: ./plan.md
"#
    )
  }

  fn issue(id: &str, state: &str) -> Issue {
    Issue {
      id: id.to_string(),
      title: "title".to_string(),
      description: String::new(),
      state: state.to_string(),
      extra_payload: serde_yaml::Mapping::new(),
    }
  }
}
