//! Span factories.
//!
//! Each helper seeds a `phase` field plus any context fields known at
//! span open time. Fields not yet known are reserved as
//! [`tracing::field::Empty`] so downstream code can fill them in via
//! `Span::record` (e.g. `session_id` once the agent reports it).
//!
//! The `phase` field is the lowest-common-denominator filter for log
//! consumers — every emitted event must carry exactly one phase value.

use tracing::Span;

use super::phase::Phase;

pub fn daemon_span() -> Span {
  tracing::info_span!("daemon", phase = Phase::Daemon.as_str())
}

pub fn issue_span(issue_id: &str) -> Span {
  tracing::info_span!("issue", phase = Phase::Dispatch.as_str(), issue_id)
}

pub fn stage_span(stage_name: &str, agent_profile: &str) -> Span {
  tracing::info_span!("stage", phase = Phase::StageRun.as_str(), stage_name, agent_profile)
}

pub fn session_span(agent: &str) -> Span {
  tracing::info_span!("session", agent)
}

#[cfg(test)]
mod tests {
  use crate::logging::tests::CaptureLayer;

  use super::*;

  use serde_json::Value;
  use tracing::{Instrument, subscriber::with_default};
  use tracing_subscriber::{Registry, layer::SubscriberExt};

  #[tokio::test]
  async fn derived_span_data_should_show_in_child_span() {
    let (layer, events) = CaptureLayer::new();

    with_default(Registry::default().with(layer), move || {
      let _issue_span = issue_span("ABC-123").entered();
      tracing::info!("test issue");

      tokio::spawn(
        async move {
          let stage_span = stage_span("plan", "codex");
          let _entered = stage_span.enter();
          tracing::info!("test session");

          let events = events.lock().unwrap();
          assert_eq!(events.len(), 2);
          let issue_event = &events[0];
          let stage_event = &events[1];

          assert_eq!(issue_event["issue_id"], Value::String("ABC-123".into()));
          assert_eq!(issue_event["phase"], Value::String(Phase::Dispatch.to_string()));
          assert_eq!(stage_event["issue_id"], Value::String("ABC-123".into()));
          assert_eq!(stage_event["phase"], Value::String(Phase::StageRun.to_string()));
          assert_eq!(stage_event["stage_name"], Value::String("plan".into()));
        }
        .in_current_span(),
      );
    });
  }
}
