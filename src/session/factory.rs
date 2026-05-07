use std::path::PathBuf;
use std::sync::Arc;

use super::{Session, SessionError};
use crate::{config::IssueStage, context::Issue, workflow::Workflow};

/// Spawn boundary between the orchestrator and `Session`. Resolving the
/// agent profile here keeps `Session::spawn`'s error surface focused on
/// runtime failures, and lets the launcher accept stage data without
/// also wiring in the agent registry.
#[derive(Clone)]
pub struct SessionFactory {
  workflow: Arc<Workflow>,
}

impl SessionFactory {
  pub fn new(workflow: Arc<Workflow>) -> SessionFactory {
    SessionFactory { workflow }
  }

  pub async fn spawn(&self, issue: Issue, stage: IssueStage) -> Result<Session, SessionError> {
    let issue_workdir = self.workflow.workspace().issue_workdir(&issue.id);
    self.spawn_named(issue, String::new(), stage, issue_workdir).await
  }

  pub async fn spawn_named(
    &self,
    issue: Issue,
    stage_name: impl Into<String>,
    stage: IssueStage,
    issue_workdir: PathBuf,
  ) -> Result<Session, SessionError> {
    let profile = match self.workflow.agents().get(&stage.agent) {
      Some(profile) => profile,
      None => {
        return Err(SessionError::ProfileNotFound {
          profile: stage.agent.clone(),
        });
      },
    };

    Session::spawn(
      Arc::clone(&self.workflow),
      issue,
      stage_name.into(),
      stage,
      issue_workdir,
      profile.clone(),
    )
    .await
  }
}
