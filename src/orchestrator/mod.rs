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
              if self.handle_event(event) {
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
        }
      }
    }
  }

  /// Returns `true` when intake has stopped. The caller still waits for
  /// `StageSessionManager` to drain before exiting.
  fn handle_event(&mut self, event: OrchestratorEvent) -> bool {
    match event {
      OrchestratorEvent::Intake(IntakeEvent::Issue(issue)) => {
        self.sessions.try_run_issue(issue);
        false
      },
      OrchestratorEvent::Intake(IntakeEvent::Failed(error)) => {
        tracing::error!(error = %error, "intake cycle failed");
        false
      },
      OrchestratorEvent::Intake(IntakeEvent::Stopped) => true,
    }
  }
}
