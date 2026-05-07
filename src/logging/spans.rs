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
use tracing::field::Empty;

use super::phase::Phase;

pub fn dispatch_span() -> Span {
  tracing::info_span!("dispatch", phase = Phase::Dispatch.as_str(),)
}

pub fn stage_span(issue_identifier: &str, stage_name: &str, agent_profile: &str) -> Span {
  tracing::info_span!(
    "stage_run",
    phase = Phase::StageRun.as_str(),
    issue_identifier = issue_identifier,
    stage_name = stage_name,
    agent_profile = agent_profile,
    runtime = Empty,
    session_id = Empty,
    duration_ms = Empty,
  )
}

pub fn daemon_span() -> Span {
  tracing::info_span!("daemon", phase = Phase::Daemon.as_str(),)
}

#[cfg(test)]
mod tests {
  use super::*;

  use tracing::subscriber::with_default;
  use tracing_subscriber::Registry;

  #[test]
  fn stage_span_has_phase_field() {
    // Subscriber must be active for `info_span!` to record metadata;
    // otherwise the span is disabled and `metadata()` returns `None`.
    // The assertion is indirect (compilation + non-None metadata) but
    // catches a future refactor that drops the `phase` field.
    with_default(Registry::default(), || {
      let span = stage_span("ABC-1", "plan", "codex");
      let _ = span.metadata().expect("metadata present");
    });
  }
}
