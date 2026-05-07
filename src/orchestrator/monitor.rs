//! Session-state observer.
//!
//! One monitor per running stage. It subscribes to the session's
//! state-change watch channel, emits a snapshot for every visible
//! transition, and returns the terminal snapshot to the launcher. The
//! monitor is read-only; it never mutates session or running state.

use tracing::Span;

use crate::session::{Session, SessionSnapshot};

use super::event::EventProducer;
use super::types::IssueStageKey;

pub(super) struct SessionMonitor {
  key: IssueStageKey,
  session: Session,
  producer: EventProducer,
}

impl SessionMonitor {
  pub(super) fn new(key: IssueStageKey, session: Session, producer: EventProducer) -> Self {
    Self { key, session, producer }
  }

  pub(super) async fn watch(self) -> SessionSnapshot {
    let mut states = self.session.subscribe_state();

    loop {
      let snapshot = self.session.snapshot();
      record_session_id(&snapshot);
      self.producer.stage_snapshot(self.key.clone(), snapshot.clone()).await;

      if snapshot.state.is_terminated() {
        return snapshot;
      }

      // `Err` means the watch channel was closed because the session
      // was dropped. Re-snapshot to capture the final state — the
      // session's last write may not have notified us yet.
      if states.changed().await.is_err() {
        return self.session.snapshot();
      }
    }
  }
}

/// Stamp the provider session id onto the active span the moment we
/// first observe it; downstream log lines then carry it without each
/// emitter having to thread it through.
fn record_session_id(snapshot: &SessionSnapshot) {
  if let Some(session_id) = snapshot.agent_session_id.as_deref() {
    Span::current().record("session_id", session_id);
  }
}
