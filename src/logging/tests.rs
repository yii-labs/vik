//! Unit tests for the logging module.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

/// Shared buffer of captured events.
pub(crate) type CapturedEvents = Arc<Mutex<Vec<serde_json::Value>>>;

/// Mirrors the production JSON layer's `flatten_event`: each event
/// captures both event-level fields and every span field in the
/// current scope, so test assertions match operator-visible logs.
pub(crate) struct CaptureLayer {
  buffer: CapturedEvents,
}

impl CaptureLayer {
  pub(crate) fn new() -> (Self, CapturedEvents) {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    (
      Self {
        buffer: Arc::clone(&buffer),
      },
      buffer,
    )
  }
}

pub(crate) fn captured_event<'event>(events: &'event [serde_json::Value], message: &str) -> &'event serde_json::Value {
  events
    .iter()
    .find(|event| event["message"] == message)
    .unwrap_or_else(|| panic!("missing captured message: {message}"))
}

pub(crate) fn captured_message_exists(events: &[serde_json::Value], message: &str) -> bool {
  events.iter().any(|event| event["message"] == message)
}

impl<S> Layer<S> for CaptureLayer
where
  S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
  fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, id: &tracing::span::Id, ctx: Context<'_, S>) {
    let span = ctx.span(id).expect("span id is valid");
    let mut visitor = FieldVisitor::default();
    attrs.record(&mut visitor);
    span.extensions_mut().insert(SpanFields(visitor.fields));
  }

  fn on_record(&self, id: &tracing::span::Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
    let span = ctx.span(id).expect("span id is valid");
    let mut ext = span.extensions_mut();
    let stored = ext.get_mut::<SpanFields>().expect("span fields recorded on new_span");
    let mut visitor = FieldVisitor::default();
    values.record(&mut visitor);
    for (k, v) in visitor.fields {
      stored.0.insert(k, v);
    }
  }

  fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
    let mut merged: HashMap<String, serde_json::Value> = HashMap::new();

    // Outer-to-inner walk: inner span fields overwrite outer ones,
    // matching tracing-subscriber's JSON layer.
    let mut span_values = Vec::new();

    if let Some(scope) = ctx.event_scope(event) {
      let spans: Vec<_> = scope.from_root().collect();
      for span in spans {
        let mut span_payload = serde_json::Map::new();
        span_payload.insert("name".to_string(), json!(span.metadata().name()));
        if let Some(fields) = span.extensions().get::<SpanFields>() {
          for (k, v) in &fields.0 {
            merged.insert(k.clone(), v.clone());
            span_payload.insert(k.clone(), v.clone());
          }
        }
        span_values.push(serde_json::Value::Object(span_payload));
      }
    }

    let mut visitor = FieldVisitor::default();
    event.record(&mut visitor);
    for (k, v) in visitor.fields {
      merged.insert(k, v);
    }

    let level = event.metadata().level().to_string().to_lowercase();
    merged.insert("level".to_string(), json!(level));
    merged.insert("target".to_string(), json!(event.metadata().target()));
    if !span_values.is_empty() {
      merged.insert("spans".to_string(), serde_json::Value::Array(span_values));
    }

    let payload = serde_json::Value::Object(merged.into_iter().collect());
    self.buffer.lock().expect("buffer mutex").push(payload);
  }
}

/// Span-attached field storage used by [`CaptureLayer`].
struct SpanFields(HashMap<String, serde_json::Value>);

/// Visitor that flattens tracing field values into JSON values.
#[derive(Default)]
struct FieldVisitor {
  fields: HashMap<String, serde_json::Value>,
}

impl Visit for FieldVisitor {
  fn record_str(&mut self, field: &Field, value: &str) {
    self.fields.insert(field.name().to_string(), json!(value));
  }

  fn record_i64(&mut self, field: &Field, value: i64) {
    self.fields.insert(field.name().to_string(), json!(value));
  }

  fn record_u64(&mut self, field: &Field, value: u64) {
    self.fields.insert(field.name().to_string(), json!(value));
  }

  fn record_f64(&mut self, field: &Field, value: f64) {
    self.fields.insert(field.name().to_string(), json!(value));
  }

  fn record_bool(&mut self, field: &Field, value: bool) {
    self.fields.insert(field.name().to_string(), json!(value));
  }

  fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
    self.fields.insert(field.name().to_string(), json!(format!("{value:?}")));
  }

  fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
    self.fields.insert(field.name().to_string(), json!(value.to_string()));
  }
}
