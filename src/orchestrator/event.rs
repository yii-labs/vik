//! Typed orchestrator event channel.
//!
//! One bounded mpsc channel carries intake signals into the orchestrator
//! main loop. Stage/session signals stay inside
//! [`super::session_manager::StageSessionManager`].

use tokio::sync::mpsc;

use crate::context::Issue;
use crate::logging::Phase;

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
}

pub(super) enum IntakeEvent {
  Issue(Issue),
  /// Recoverable error during one cycle — the loop keeps going.
  Failed(String),
  /// Natural end (max iterations or cooperative shutdown). Triggers the
  /// main loop's drain check.
  Stopped,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn producer_delivers_intake_events_in_order() {
    let (producer, mut consumer) = event_channel();

    producer.intake_failed("pull failed").await;
    producer.intake_stopped().await;

    match consumer.recv().await.expect("first event") {
      OrchestratorEvent::Intake(IntakeEvent::Failed(error)) => assert_eq!(error, "pull failed"),
      _ => panic!("expected intake failure"),
    }

    match consumer.recv().await.expect("second event") {
      OrchestratorEvent::Intake(IntakeEvent::Stopped) => {},
      _ => panic!("expected intake stopped"),
    }
  }
}
