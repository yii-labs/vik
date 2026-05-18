//! Channel-driven runtime orchestrator.
//!
//! The top-level orchestrator owns intake, shutdown, and drain. Stage
//! matching, issue preparation, hook execution, and session state live
//! behind [`session_manager::StageSessionManager`].

mod event;
mod intake;
mod session_manager;

use std::sync::Arc;

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::logging::Phase;
use crate::workflow::Workflow;

use self::event::{IntakeEvent, OrchestratorEvent, event_channel};
use self::intake::IntakeLoop;
use self::session_manager::StageSessionManager;

#[derive(Debug, Error)]
pub enum OrchestratorError {
  #[error("orchestrator event channel closed while work was still running")]
  EventChannelClosed,
}

pub struct Orchestrator {
  workflow: Arc<Workflow>,
  sessions: StageSessionManager,
}

impl Orchestrator {
  pub fn new(workflow: Workflow) -> Self {
    let workflow = Arc::new(workflow);
    let sessions = StageSessionManager::new(Arc::clone(&workflow));

    Self { workflow, sessions }
  }

  /// Drive intake until shutdown or natural drain.
  ///
  /// Natural exit requires intake to stop and all stage sessions to drain.
  /// Hard shutdown cancels sessions and aborts intake without waiting for
  /// the manager channel to drain.
  pub async fn run(&mut self, shutdown: CancellationToken) -> Result<(), OrchestratorError> {
    let (producer, mut consumer) = event_channel();
    let intake = IntakeLoop::new(Arc::clone(&self.workflow), producer);
    let intake_handle = intake.start(shutdown.clone());
    let mut intake_stopped = false;
    let mut intake_closed = false;

    loop {
      if intake_stopped && self.sessions.is_empty() {
        return Ok(());
      }

      tokio::select! {
        biased;

        _ = shutdown.cancelled() => {
          self.sessions.cancel_all().await;
          intake_handle.abort();
          return Ok(());
        }

        event = consumer.recv(), if !intake_closed => {
          match event {
            Some(event) => {
              if self.handle_event(event).await {
                intake_stopped = true;
              }
            }
            None => {
              intake_closed = true;
              if self.sessions.is_empty() {
                return Ok(());
              }
              if !intake_stopped {
                return Err(OrchestratorError::EventChannelClosed);
              }
            },
          }
        }

        received = self.sessions.recv(), if !self.sessions.is_empty() => {
          if received.is_none() {
            return Err(OrchestratorError::EventChannelClosed);
          }

          let _ = self.sessions.handle_received_event().await;
        }
      }
    }
  }

  /// Returns `true` when intake has stopped. The caller still waits for
  /// `StageSessionManager` to drain before exiting.
  async fn handle_event(&mut self, event: OrchestratorEvent) -> bool {
    match event {
      OrchestratorEvent::Intake(IntakeEvent::Issue(issue)) => {
        self.sessions.try_spawn(issue).await;
        false
      },
      OrchestratorEvent::Intake(IntakeEvent::Failed(error)) => {
        tracing::error!(phase = %Phase::Intake, error = %error, "intake cycle failed");
        false
      },
      OrchestratorEvent::Intake(IntakeEvent::Stopped) => true,
    }
  }
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::time::Duration;

  use tokio::time::timeout;

  use super::*;
  use crate::context::Issue;
  use crate::workflow::Workflow;

  #[tokio::test]
  async fn intake_issue_event_runs_issue_setup_inside_session_manager() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("workspace");
    let workflow_path = temp.path().join("workflow.yml");
    let workflow = workflow_fixture_with_path(10, Some("echo ok >> after_create.log"), &root, workflow_path);
    let mut orchestrator = Orchestrator::new(workflow);

    let intake_stopped = orchestrator
      .handle_event(OrchestratorEvent::Intake(IntakeEvent::Issue(issue("ABC-1", "todo"))))
      .await;
    assert!(!intake_stopped);

    timeout(Duration::from_secs(2), recv_until_drained(&mut orchestrator.sessions))
      .await
      .expect("session manager drains")
      .expect("drained event");

    let issue_workdir = orchestrator.workflow.workspace().issue_workdir("ABC-1");
    assert_eq!(
      fs::read_to_string(issue_workdir.join("after_create.log"))
        .expect("after_create hook wrote log")
        .trim(),
      "ok"
    );
    assert!(
      !orchestrator
        .workflow
        .workspace()
        .issue_sessions_dir("ABC-1")
        .join("after-create.done")
        .exists(),
      "issue setup must not create stage/session marker files"
    );
  }

  #[tokio::test]
  async fn intake_stopped_event_marks_intake_stopped() {
    let mut orchestrator = Orchestrator::new(workflow_fixture(1, None));

    let intake_stopped = orchestrator.handle_event(OrchestratorEvent::Intake(IntakeEvent::Stopped)).await;

    assert!(intake_stopped);
  }

  fn workflow_fixture(max_issue_concurrency: u32, after_create: Option<&str>) -> Workflow {
    let mut builder = Workflow::builder()
      .max_issue_concurrency(max_issue_concurrency)
      .add_stage("plan", "todo", "./plan.md")
      .add_stage("implement", "todo", "./implement.md")
      .workspace_root("workspace");

    if let Some(after_create) = after_create {
      builder = builder.after_issue_workdir_create_hook(after_create);
    }

    builder.build()
  }

  fn workflow_fixture_with_path(
    max_issue_concurrency: u32,
    after_create: Option<&str>,
    root: &std::path::Path,
    workflow_path: std::path::PathBuf,
  ) -> Workflow {
    let mut builder = Workflow::builder()
      .max_issue_concurrency(max_issue_concurrency)
      .add_stage("plan", "todo", "./plan.md")
      .add_stage("implement", "todo", "./implement.md")
      .workspace_root(root)
      .workflow_path(workflow_path);

    if let Some(after_create) = after_create {
      builder = builder.after_issue_workdir_create_hook(after_create);
    }

    builder.build()
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

  async fn recv_until_drained(manager: &mut StageSessionManager) -> Option<session_manager::StageSessionEvent> {
    loop {
      manager.recv().await?;
      if let Some(event) = manager.handle_received_event().await {
        return Some(event);
      }
    }
  }
}
