//! Typed orchestrator event channel.
//!
//! One bounded mpsc channel carries every signal: producers are cloned
//! into background tasks, the single consumer lives on the main loop.
//! Event-style coordination (instead of callbacks) keeps every state
//! transition visible at one site — the only place [`super::running::RunningMap`]
//! is mutated.

use tokio::sync::mpsc;

use crate::context::{Issue, IssueStage, IssueStageKey};
use crate::logging::Phase;
use crate::session::{Session, SessionSnapshot};

/// Bounded so a slow main loop applies backpressure to producers. 256 is
/// large enough to swallow a normal intake burst without forcing intake
/// to await mid-cycle.
const EVENT_BUFFER: usize = 256;

#[derive(Clone)]
pub(super) struct EventProducer {
  sender: mpsc::Sender<OrchestratorEvent>,
}

impl EventProducer {
  pub(super) async fn intake_issue(&self, issue: Issue) {
    self.send(OrchestratorEvent::Intake(IntakeEvent::Issue(issue))).await;
  }

  pub(super) async fn intake_failed(&self, error: impl ToString) {
    self
      .send(OrchestratorEvent::Intake(IntakeEvent::Failed(error.to_string())))
      .await;
  }

  pub(super) async fn intake_stopped(&self) {
    self.send(OrchestratorEvent::Intake(IntakeEvent::Stopped)).await;
  }

  pub(super) async fn issue_ready(&self, issue_stages: Vec<IssueStage>) {
    self
      .send(OrchestratorEvent::Stage(StageEvent::IssueReady { issue_stages }))
      .await;
  }

  pub(super) async fn stage_started(&self, issue_stage: IssueStage, session: Session) {
    self
      .send(OrchestratorEvent::Stage(StageEvent::Started {
        issue_stage: Box::new(issue_stage),
        session,
      }))
      .await;
  }

  pub(super) async fn stage_snapshot(&self, key: IssueStageKey, snapshot: SessionSnapshot) {
    self
      .send(OrchestratorEvent::Stage(StageEvent::Snapshot { key, snapshot }))
      .await;
  }

  pub(super) async fn stage_terminal(&self, key: IssueStageKey, snapshot: SessionSnapshot) {
    self
      .send(OrchestratorEvent::Stage(StageEvent::Terminal { key, snapshot }))
      .await;
  }

  /// Pre-session failure path: there is no `Session`, only the reserved
  /// key, so the main loop can release the reservation.
  pub(super) async fn stage_failed(&self, key: IssueStageKey, error: impl ToString) {
    self
      .send(OrchestratorEvent::Stage(StageEvent::Failed {
        key,
        error: error.to_string(),
      }))
      .await;
  }

  async fn send(&self, event: OrchestratorEvent) {
    if self.sender.send(event).await.is_err() {
      tracing::debug!(phase = %Phase::Dispatch, "orchestrator event receiver dropped");
    }
  }
}

pub(super) struct EventConsumer {
  receiver: mpsc::Receiver<OrchestratorEvent>,
}

impl EventConsumer {
  pub(super) async fn recv(&mut self) -> Option<OrchestratorEvent> {
    self.receiver.recv().await
  }
}

pub(super) fn event_channel() -> (EventProducer, EventConsumer) {
  let (sender, receiver) = mpsc::channel(EVENT_BUFFER);
  (EventProducer { sender }, EventConsumer { receiver })
}

// TODO: FIXME
#[allow(clippy::large_enum_variant)]
pub(super) enum OrchestratorEvent {
  Intake(IntakeEvent),
  Stage(StageEvent),
}

pub(super) enum IntakeEvent {
  Issue(Issue),
  /// Recoverable error during one cycle — the loop keeps going.
  Failed(String),
  /// Natural end (max iterations or cooperative shutdown). Triggers the
  /// main loop's drain check.
  Stopped,
}

// TODO: FIXME
#[allow(clippy::large_enum_variant)]
pub(super) enum StageEvent {
  IssueReady {
    issue_stages: Vec<IssueStage>,
  },
  Started {
    issue_stage: Box<IssueStage>,
    session: Session,
  },
  Snapshot {
    key: IssueStageKey,
    snapshot: SessionSnapshot,
  },
  Terminal {
    key: IssueStageKey,
    snapshot: SessionSnapshot,
  },
  Failed {
    key: IssueStageKey,
    error: String,
  },
}
