use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::config::IssueStage as StageSchema;
use crate::hooks::HookError;
use crate::workflow::Workflow;

use super::Issue;

#[derive(Debug, Error)]
pub enum IssueRunError {
  #[error("failed to create issue workdir `{path}`: {source}")]
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
  workdir: PathBuf,
}

impl Serialize for IssueRun {
  fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
  where
    S: serde::Serializer,
  {
    let json = serde_json::to_value(&self.issue).map_err(serde::ser::Error::custom)?;

    let mut issue = serde_json::Map::new();
    if let serde_json::Value::Object(issue_map) = json {
      for (k, v) in issue_map {
        issue.insert(k, v);
      }
      issue.insert("workdir".into(), self.workdir.to_string_lossy().into());
    }

    let mut root = serde_json::Map::new();
    root.insert("issue".into(), issue.into());
    root.insert(
      "workflow_path".into(),
      self.workflow().workflow_path().to_string_lossy().into(),
    );
    root.insert(
      "workspace_root".into(),
      self.workflow().workspace().root().to_string_lossy().into(),
    );

    root.serialize(serializer)
  }
}

impl IssueRun {
  pub fn new(workflow: Arc<Workflow>, issue: Issue) -> Self {
    let workdir = workflow.workspace().issue_workdir(&issue.id);

    Self {
      workflow,
      issue,
      workdir,
    }
  }

  pub fn workflow(&self) -> &Workflow {
    self.workflow.as_ref()
  }

  pub fn id(&self) -> &str {
    &self.issue.id
  }

  pub fn issue(&self) -> &Issue {
    &self.issue
  }

  pub fn workdir(&self) -> &Path {
    &self.workdir
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
    match tokio::fs::metadata(&self.workdir).await {
      Ok(metadata) if metadata.is_dir() => {
        tracing::debug!(path = %self.workdir.display(), "issue workdir already exists; skipping creation");
        return Ok(());
      },
      Ok(_) => {
        return Err(IssueRunError::CreateWorkspace {
          path: self.workdir.clone(),
          source: std::io::Error::other("path exists but is not a directory"),
        });
      },
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
      Err(e) => {
        return Err(IssueRunError::CreateWorkspace {
          path: self.workdir.clone(),
          source: e,
        });
      },
    };

    tokio::fs::create_dir_all(&self.workdir)
      .await
      .map_err(|source| IssueRunError::CreateWorkspace {
        path: self.workdir.clone(),
        source,
      })?;

    tracing::debug!(path = %self.workdir.display(), "created issue workdir");

    self
      .workflow()
      .hooks()
      .after_issue_workdir_created(self, &self.workflow().schema().issue.hooks.after_create)
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
  issue: Arc<IssueRun>,
  name: String,
  schema: StageSchema,
  log_file: PathBuf,
}

impl Serialize for IssueStage {
  fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
  where
    S: serde::Serializer,
  {
    self.issue.serialize(serializer)
  }
}

impl IssueStage {
  pub fn new(issue: Arc<IssueRun>, name: String, stage_schema: StageSchema) -> Self {
    // State prefix is for human eyeballing; UUIDv7 keeps names unique
    // and sorts runs within one state. Provider session ids land inside
    // the JSONL, not in the filename, because they are not always known
    // when the file must be created.
    let log_file =
      issue
        .workflow()
        .workspace()
        .issue_sessions_dir(issue.id())
        .join(format!("{}-{}.jsonl", &name, Uuid::now_v7()));

    Self {
      issue,
      name,
      schema: stage_schema,
      log_file,
    }
  }

  pub fn workflow(&self) -> &Workflow {
    self.issue.workflow()
  }

  pub fn issue(&self) -> &Issue {
    self.issue.issue()
  }

  pub fn workdir(&self) -> &Path {
    self.issue.workdir()
  }

  pub fn log_file(&self) -> &Path {
    &self.log_file
  }

  pub fn stage_name(&self) -> &str {
    &self.name
  }

  pub fn stage(&self) -> &StageSchema {
    &self.schema
  }

  pub fn key(&self) -> IssueStageKey {
    IssueStageKey::new(self.issue().id.clone(), self.name.clone())
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

    std::fs::create_dir_all(issue_run.workdir()).expect("issue workdir exists");

    issue_run.prepare().await.expect("prepare succeeds");

    assert!(
      !issue_run.workdir().join("after_create.log").exists(),
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
