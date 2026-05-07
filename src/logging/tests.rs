//! Unit tests for the logging module.
//!
//! The acceptance criteria from issue 0002 map onto the tests below:
//!
//! - Span field propagation — [`span_fields_propagate_into_event_json`].
//! - ERROR-only appender routing — [`only_error_events_reach_error_appender`].
//! - Retention — covered in [`super::retention::tests`].
//! - Workspace-root auto-create — covered in
//!   [`crate::workspace::root::tests`].

use std::io::Write;
use std::sync::{Arc, Mutex};

use tracing::subscriber::with_default;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use super::layers::CaptureLayer;
use super::spans::stage_span;

/// A Write impl that appends to an in-memory buffer. Used as the
/// `with_writer` target so the ERROR-appender routing test can read the
/// bytes back without touching a tempdir.
#[derive(Clone, Default)]
struct BufferWriter {
  inner: Arc<Mutex<Vec<u8>>>,
}

impl BufferWriter {
  fn new() -> Self {
    Self::default()
  }

  fn text(&self) -> String {
    let lock = self.inner.lock().expect("buffer lock");
    String::from_utf8_lossy(&lock).into_owned()
  }
}

impl Write for BufferWriter {
  fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
    self.inner.lock().expect("buffer lock").extend_from_slice(buf);
    Ok(buf.len())
  }

  fn flush(&mut self) -> std::io::Result<()> {
    Ok(())
  }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
  type Writer = BufferWriter;

  fn make_writer(&'a self) -> Self::Writer {
    self.clone()
  }
}

/// An event emitted inside a `stage_span` must carry the span's structured
/// fields (phase, issue_identifier, stage_name, agent_profile) in the JSON
/// payload. Callers downstream rely on that so a single
/// `issue_identifier="ABC-1"` filter reaches every line the stage produced.
#[test]
fn span_fields_propagate_into_event_json() {
  let (capture, buffer) = CaptureLayer::new();
  let subscriber = Registry::default().with(capture);

  with_default(subscriber, || {
    let span = stage_span("ABC-1", "plan", "codex");
    let _enter = span.enter();
    tracing::info!(event_kind = "stage_started", "starting stage");
  });

  let events = buffer.lock().expect("capture buffer");
  assert!(!events.is_empty(), "at least one event should have fired");

  let event = &events[0];
  assert_eq!(event["phase"], serde_json::Value::String("stage_run".into()));
  assert_eq!(event["issue_identifier"], serde_json::Value::String("ABC-1".into()));
  assert_eq!(event["stage_name"], serde_json::Value::String("plan".into()));
  assert_eq!(event["agent_profile"], serde_json::Value::String("codex".into()));
  assert_eq!(event["event_kind"], serde_json::Value::String("stage_started".into()));
  assert_eq!(event["level"], serde_json::Value::String("info".into()));
}

/// The ERROR-only file appender must drop INFO events and record ERROR
/// events. We build a local subscriber that routes output to an
/// in-memory buffer with an `error` filter layered on top, exactly the
/// way `init` composes the real error layer.
#[test]
fn only_error_events_reach_error_appender() {
  let writer = BufferWriter::new();
  let error_layer = tracing_subscriber::fmt::layer()
    .json()
    .with_writer(writer.clone())
    .with_filter(EnvFilter::new("error"));

  let subscriber = Registry::default().with(error_layer);

  with_default(subscriber, || {
    tracing::info!("info should be filtered out");
    tracing::error!("hook blew up");
    tracing::warn!("warn should be filtered out");
  });

  let text = writer.text();
  assert!(
    text.contains("hook blew up"),
    "error event should reach the appender; buffer was: {text}"
  );
  assert!(
    !text.contains("info should be filtered out"),
    "info event leaked into the error appender; buffer was: {text}"
  );
  assert!(
    !text.contains("warn should be filtered out"),
    "warn event leaked into the error appender; buffer was: {text}"
  );
}
