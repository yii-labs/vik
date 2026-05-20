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

use crate::context::Issue;
use crate::context::Issues;
use crate::workflow::Workflow;

use self::event::{EventConsumer, EventProducer, IntakeEvent, OrchestratorEvent, event_channel};
use self::intake::IntakeLoop;
use self::session_manager::StageSessionManager;

#[derive(Debug, Error)]
pub enum OrchestratorError {
  #[error("orchestrator event channel closed while work was still running")]
  EventChannelClosed,
}

#[derive(Debug, Error)]
#[error("orchestrator issue ingress closed")]
pub struct IssueIngressError;

#[derive(Clone)]
pub struct IssueIngress {
  producer: EventProducer,
}

impl IssueIngress {
  pub async fn enqueue_issue(&self, issue: Issue) -> Result<(), IssueIngressError> {
    self.producer.external_issue(issue).await.map_err(|()| IssueIngressError)
  }

  pub async fn enqueue_issues(&self, issues: Issues) -> Result<(), IssueIngressError> {
    for issue in issues.iter().cloned() {
      self.enqueue_issue(issue).await?;
    }
    Ok(())
  }
}

pub struct Orchestrator {
  workflow: Arc<Workflow>,
  sessions: StageSessionManager,
  producer: Option<EventProducer>,
  consumer: EventConsumer,
}

impl Orchestrator {
  pub fn new(workflow: Workflow) -> Self {
    let workflow = Arc::new(workflow);
    let sessions = StageSessionManager::new(Arc::clone(&workflow));
    let (producer, consumer) = event_channel();

    Self {
      workflow,
      sessions,
      producer: Some(producer),
      consumer,
    }
  }

  pub fn issue_ingress(&self) -> IssueIngress {
    IssueIngress {
      producer: self.producer.as_ref().expect("issue ingress already taken").clone(),
    }
  }

  #[cfg(test)]
  pub(crate) async fn recv_issue_for_test(&mut self) -> Option<Issue> {
    loop {
      match self.consumer.recv().await? {
        OrchestratorEvent::Intake(IntakeEvent::Issue(issue)) => return Some(issue),
        OrchestratorEvent::Intake(IntakeEvent::Failed(_) | IntakeEvent::Stopped) => {},
      }
    }
  }

  #[cfg(test)]
  pub(crate) async fn recv_issue_now_for_test(&mut self) -> Option<Issue> {
    match self.consumer.try_recv() {
      Some(OrchestratorEvent::Intake(IntakeEvent::Issue(issue))) => Some(issue),
      Some(OrchestratorEvent::Intake(IntakeEvent::Failed(_) | IntakeEvent::Stopped)) | None => None,
    }
  }

  /// Drive intake until shutdown or natural drain.
  ///
  /// Natural exit requires intake to stop and all stage sessions to drain.
  /// Hard shutdown cancels sessions and aborts intake without waiting for
  /// the manager channel to drain.
  pub async fn run(&mut self, shutdown: CancellationToken) -> Result<(), OrchestratorError> {
    self.run_inner(shutdown, false).await
  }

  /// Drive intake while an external source, such as HTTP webhook intake,
  /// can enqueue issues through [`IssueIngress`].
  pub async fn run_with_external_intake(&mut self, shutdown: CancellationToken) -> Result<(), OrchestratorError> {
    self.run_inner(shutdown, true).await
  }

  async fn run_inner(&mut self, shutdown: CancellationToken, external_intake: bool) -> Result<(), OrchestratorError> {
    let has_pull = self.workflow.schema().issues.pull.is_some();
    if !has_pull && !external_intake && self.sessions.is_empty() {
      return Ok(());
    }

    let producer = self.producer.take().ok_or(OrchestratorError::EventChannelClosed)?;
    let intake_handle =
      has_pull.then(|| IntakeLoop::new(Arc::clone(&self.workflow), producer.clone()).start(shutdown.clone()));
    drop(producer);

    let mut intake_stopped = !has_pull;
    let mut intake_closed = false;

    loop {
      if intake_stopped && !external_intake && self.sessions.is_empty() {
        return Ok(());
      }

      tokio::select! {
        biased;

        _ = shutdown.cancelled() => {
          self.sessions.cancel_all().await;
          if let Some(intake_handle) = &intake_handle {
            intake_handle.abort();
          }
          return Ok(());
        }

        event = self.consumer.recv(), if !intake_closed => {
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
        tracing::info_span!("intake").in_scope(|| {
          tracing::error!(error = %error, "intake cycle failed");
        });
        false
      },
      OrchestratorEvent::Intake(IntakeEvent::Stopped) => true,
    }
  }
}

#[cfg(test)]
mod tests {
  use tracing_subscriber::{Registry, layer::SubscriberExt};

  use super::*;
  use crate::context::Issue;
  use crate::logging::tests::{CaptureLayer, captured_event};

  #[test]
  fn intake_failure_logs_inside_intake_span() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);
    let _default = tracing::subscriber::set_default(subscriber);

    let mut orchestrator = Orchestrator::new(Workflow::builder().workspace_root("workspace").build());

    let stopped = orchestrator.handle_event(OrchestratorEvent::Intake(IntakeEvent::Failed(
      "pull failed".to_string(),
    )));

    assert!(!stopped);
    let events = events.lock().expect("events mutex");
    let event = captured_event(&events, "intake cycle failed");
    assert_eq!(event["spans"][0]["name"], "intake");
    assert!(event.get("phase").is_none());
  }

  #[tokio::test]
  async fn issue_ingress_sends_issue_through_orchestrator_event_channel() {
    let mut orchestrator = Orchestrator::new(Workflow::builder().workspace_root("workspace").build());
    let ingress = orchestrator.issue_ingress();

    ingress.enqueue_issue(issue("WEB-1")).await.expect("issue enqueued");

    match orchestrator.consumer.recv().await.expect("event") {
      OrchestratorEvent::Intake(IntakeEvent::Issue(issue)) => {
        assert_eq!(issue.id, "WEB-1");
      },
      _ => panic!("expected intake issue"),
    }
  }

  #[tokio::test]
  async fn pull_template_failure_does_not_hang_without_external_intake() {
    let mut orchestrator = Orchestrator::new(
      Workflow::builder()
        .pull_command("{{ missing_pull_template_value }}")
        .workspace_root("workspace")
        .build(),
    );

    tokio::time::timeout(
      std::time::Duration::from_secs(2),
      orchestrator.run(CancellationToken::new()),
    )
    .await
    .expect("orchestrator should stop after intake failure")
    .expect("orchestrator exits cleanly");
  }

  fn issue(id: &str) -> Issue {
    Issue {
      id: id.to_string(),
      title: "Webhook issue".to_string(),
      description: String::new(),
      state: "todo".to_string(),
      extra_payload: serde_yaml::Mapping::new(),
    }
  }
}
