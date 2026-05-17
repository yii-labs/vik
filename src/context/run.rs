use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::config::IssueStageSchema as StageSchema;
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
      .filter(|stage| stage.when.state == issue_run.issue.state)
      .map(|stage| IssueStage::new(Arc::clone(&issue_run), stage.clone()))
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
  schema: StageSchema,
  log_file: PathBuf,
}

impl Serialize for IssueStage {
  fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
  where
    S: serde::Serializer,
  {
    let mut root = serde_json::to_value(self.issue.as_ref()).map_err(serde::ser::Error::custom)?;
    let issue = root
      .get_mut("issue")
      .and_then(serde_json::Value::as_object_mut)
      .ok_or_else(|| serde::ser::Error::custom("issue context must serialize as object"))?;
    let tracker_stage = issue.remove("stage");
    let mut stage = serde_json::Map::new();
    stage.insert("name".into(), self.stage_name().into());
    if let Some(value) = tracker_stage {
      stage.insert("value".into(), value);
    }
    issue.insert("stage".into(), serde_json::Value::Object(stage));

    root.serialize(serializer)
  }
}

impl IssueStage {
  pub fn new(issue: Arc<IssueRun>, stage_schema: StageSchema) -> Self {
    // State prefix is for human eyeballing; UUIDv7 keeps names unique
    // and sorts runs within one state. Provider session ids land inside
    // the JSONL, not in the filename, because they are not always known
    // when the file must be created.
    let log_file = issue.workflow().workspace().issue_sessions_dir(issue.id()).join(format!(
      "{}-{}.jsonl",
      &stage_schema.name,
      Uuid::now_v7()
    ));

    Self {
      issue,
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
    &self.schema.name
  }

  pub fn stage(&self) -> &StageSchema {
    &self.schema
  }

  pub fn key(&self) -> IssueStageKey {
    IssueStageKey::new(self.issue().id.clone(), self.schema.name.clone())
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use super::*;
  use crate::template::JinjaRenderer;
  use crate::workflow::Workflow;

  #[test]
  fn issue_run_serializes_template_context_shape() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow = Arc::new(workflow_fixture(
      &temp.path().join("workspace"),
      temp.path().join("workflow.yml"),
      "echo created",
    ));
    let issue_run = IssueRun::new(Arc::clone(&workflow), issue_with_extra("ABC-1", "todo"));

    let context = serde_json::to_value(&issue_run).expect("issue run serializes");

    assert_eq!(
      context["workflow_path"].as_str(),
      Some(workflow.workflow_path().to_string_lossy().as_ref())
    );
    assert_eq!(
      context["workspace_root"].as_str(),
      Some(workflow.workspace().root().to_string_lossy().as_ref())
    );
    assert_eq!(context["issue"]["id"], "ABC-1");
    assert_eq!(context["issue"]["title"], "title");
    assert_eq!(context["issue"]["description"], "");
    assert_eq!(context["issue"]["state"], "todo");
    assert_eq!(
      context["issue"]["workdir"].as_str(),
      Some(issue_run.workdir().to_string_lossy().as_ref())
    );
    assert_eq!(context["issue"]["priority"], "high");
    assert!(context.get("id").is_none());
    assert!(context.get("workdir").is_none());
  }

  #[test]
  fn issue_run_serialized_context_renders_jinja_template() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow = Arc::new(workflow_fixture(
      &temp.path().join("workspace"),
      temp.path().join("workflow.yml"),
      "echo created",
    ));
    let issue_run = IssueRun::new(Arc::clone(&workflow), issue_with_extra("ABC-1", "todo"));

    let rendered = JinjaRenderer::new()
      .render(
        "{{ issue.id }}|{{ issue.priority }}|{{ issue.workdir }}|{{ workflow_path }}|{{ workspace_root }}",
        &issue_run,
      )
      .expect("issue run context renders");

    assert_eq!(
      rendered,
      format!(
        "ABC-1|high|{}|{}|{}",
        issue_run.workdir().display(),
        workflow.workflow_path().display(),
        workflow.workspace().root().display()
      )
    );
  }

  #[test]
  fn issue_stage_serializes_stage_context_under_issue() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow = Arc::new(
      Workflow::builder()
        .workflow_path(temp.path().join("workflow.yml"))
        .workspace_root(temp.path().join("workspace"))
        .add_stage("plan", "todo", "./plan.md")
        .build(),
    );
    let issue_run = Arc::new(IssueRun::new(Arc::clone(&workflow), issue_with_extra("ABC-1", "todo")));
    let stage = IssueRun::matching_stages(Arc::clone(&issue_run))
      .into_iter()
      .next()
      .expect("stage matches issue state");

    let context = serde_json::to_value(&stage).expect("issue stage serializes");
    assert_eq!(context["issue"]["stage"]["name"], "plan");
    assert!(context.get("stage").is_none());
    assert!(
      serde_json::to_value(issue_run.as_ref()).expect("issue run serializes")["issue"]
        .get("stage")
        .is_none()
    );

    let rendered = JinjaRenderer::new()
      .render(
        "{{ issue.id }}|{{ issue.priority }}|{{ issue.stage.name }}|{{ issue.workdir }}|{{ workflow_path }}|{{ workspace_root }}",
        &stage,
      )
      .expect("issue stage context renders");

    assert_eq!(
      rendered,
      format!(
        "ABC-1|high|plan|{}|{}|{}",
        stage.workdir().display(),
        workflow.workflow_path().display(),
        workflow.workspace().root().display()
      )
    );
  }

  #[test]
  fn issue_stage_serialization_preserves_tracker_stage_payload_as_value() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow = Arc::new(
      Workflow::builder()
        .workflow_path(temp.path().join("workflow.yml"))
        .workspace_root(temp.path().join("workspace"))
        .add_stage("plan", "todo", "./plan.md")
        .build(),
    );
    let mut issue = issue_with_extra("ABC-1", "todo");
    issue.extra_payload.insert(
      serde_yaml::Value::String("stage".to_string()),
      serde_yaml::Value::String("tracker-stage".to_string()),
    );
    let issue_run = Arc::new(IssueRun::new(Arc::clone(&workflow), issue));
    let stage = IssueRun::matching_stages(issue_run)
      .into_iter()
      .next()
      .expect("stage matches issue state");

    let context = serde_json::to_value(&stage).expect("issue stage serializes");

    assert_eq!(context["issue"]["stage"]["name"], "plan");
    assert_eq!(context["issue"]["stage"]["value"], "tracker-stage");

    let rendered = JinjaRenderer::new()
      .render("{{ issue.stage.name }}|{{ issue.stage.value }}", &stage)
      .expect("stage context renders");

    assert_eq!(rendered, "plan|tracker-stage");
  }

  #[test]
  fn matching_stages_preserve_workflow_author_order() {
    let workflow = Arc::new(
      Workflow::builder()
        .add_stage("plan", "todo", "./plan.md")
        .add_stage("implement", "todo", "./implement.md")
        .add_stage("merge", "merging", "./merge.md")
        .build(),
    );
    let issue_run = Arc::new(IssueRun::new(Arc::clone(&workflow), issue("ABC-1", "todo")));

    let matching_stages = IssueRun::matching_stages(issue_run);

    assert_eq!(
      matching_stages
        .iter()
        .map(|issue_stage| issue_stage.stage_name())
        .collect::<Vec<_>>(),
      ["plan", "implement"]
    );
  }

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
    Workflow::builder()
      .workflow_path(workflow_path)
      .workspace_root(root)
      .after_issue_workdir_create_hook(after_create)
      .build()
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

  fn issue_with_extra(id: &str, state: &str) -> Issue {
    let mut issue = issue(id, state);
    issue.extra_payload.insert(
      serde_yaml::Value::String("priority".to_string()),
      serde_yaml::Value::String("high".to_string()),
    );
    issue
  }
}
