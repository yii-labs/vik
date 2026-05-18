use std::sync::Arc;

use super::{Session, SessionCommandSender, SessionError, SessionStateReceiver};
use crate::{context::IssueStage, workflow::Workflow};
use tokio_util::sync::CancellationToken;

/// Spawn boundary between the stage-session manager and session task.
/// Resolving the agent profile here keeps caller setup focused on
/// workflow data and leaves runtime failures to the session state stream.
#[derive(Clone)]
pub struct SessionFactory {
  workflow: Arc<Workflow>,
}

impl SessionFactory {
  pub fn new(workflow: Arc<Workflow>) -> SessionFactory {
    SessionFactory { workflow }
  }

  pub fn spawn_stage(
    &self,
    issue_stage: IssueStage,
    shutdown: CancellationToken,
  ) -> Result<(SessionCommandSender, SessionStateReceiver), SessionError> {
    let stage = issue_stage.stage();
    let profile = match self.workflow.agents().get(&stage.agent) {
      Some(profile) => profile,
      None => {
        return Err(SessionError::ProfileNotFound {
          profile: stage.agent.clone(),
        });
      },
    };

    Ok(Session::spawn(issue_stage, profile.clone(), shutdown))
  }
}
