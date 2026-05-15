use std::sync::Arc;

use super::{Session, SessionError};
use crate::{context::IssueStage, workflow::Workflow};

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

  pub async fn spawn_stage(&self, issue_stage: IssueStage) -> Result<Session, SessionError> {
    let stage = issue_stage.stage();
    let profile = match self.workflow.agents().get(&stage.agent) {
      Some(profile) => profile,
      None => {
        return Err(SessionError::ProfileNotFound {
          profile: stage.agent.clone(),
        });
      },
    };

    Session::spawn(Arc::clone(&self.workflow), issue_stage, profile.clone()).await
  }
}
