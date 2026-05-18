use std::fmt::Write as _;
use std::io;

use chrono::{SecondsFormat, Utc};
use indexmap::IndexMap;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

pub(super) fn layer<W>(writer: W) -> StdoutLayer<W> {
  StdoutLayer { writer }
}

pub(super) struct StdoutLayer<W> {
  writer: W,
}

impl<S, W> Layer<S> for StdoutLayer<W>
where
  S: Subscriber + for<'lookup> LookupSpan<'lookup>,
  W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
  fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, id: &tracing::span::Id, ctx: Context<'_, S>) {
    let Some(span) = ctx.span(id) else {
      return;
    };

    let mut visitor = FieldVisitor::default();
    attrs.record(&mut visitor);
    span.extensions_mut().insert(StdoutFields(visitor.fields));
  }

  fn on_record(&self, id: &tracing::span::Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
    let Some(span) = ctx.span(id) else {
      return;
    };

    let mut visitor = FieldVisitor::default();
    values.record(&mut visitor);

    let mut extensions = span.extensions_mut();
    let Some(stored) = extensions.get_mut::<StdoutFields>() else {
      extensions.insert(StdoutFields(visitor.fields));
      return;
    };

    for (key, value) in visitor.fields {
      stored.0.insert(key, value);
    }
  }

  fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
    let line = format_event(event, ctx);
    let mut writer = self.writer.make_writer_for(event.metadata());
    if let Err(err) = io::Write::write_all(&mut writer, line.as_bytes()) {
      eprintln!("[tracing-subscriber] Unable to write an event to the stdout log writer: {err}");
    }
  }
}

fn format_event<S>(event: &Event<'_>, ctx: Context<'_, S>) -> String
where
  S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
  let mut span_names = Vec::new();
  let mut fields = IndexMap::new();

  if let Some(scope) = ctx.event_scope(event) {
    for span in scope.from_root() {
      span_names.push(span.metadata().name());
      if let Some(span_fields) = span.extensions().get::<StdoutFields>() {
        for (key, value) in &span_fields.0 {
          fields.insert(key.clone(), value.clone());
        }
      }
    }
  }

  let mut visitor = FieldVisitor::default();
  event.record(&mut visitor);
  for (key, value) in visitor.fields {
    fields.insert(key, value);
  }

  let message = fields
    .shift_remove("message")
    .unwrap_or_else(|| event.metadata().name().to_string());

  let mut line = String::new();
  write!(
    &mut line,
    "{} {} ",
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
    event.metadata().level()
  )
  .expect("write to String");
  if !span_names.is_empty() {
    write!(&mut line, "{}: ", span_names.join(":")).expect("write to String");
  }
  write!(&mut line, "{}: {message}", event.metadata().target()).expect("write to String");
  for (key, value) in fields {
    write!(&mut line, " {key}={value}").expect("write to String");
  }
  line.push('\n');
  line
}

struct StdoutFields(IndexMap<String, String>);

#[derive(Default)]
struct FieldVisitor {
  fields: IndexMap<String, String>,
}

impl Visit for FieldVisitor {
  fn record_str(&mut self, field: &Field, value: &str) {
    let name = field_name(field);
    if name == "message" {
      self.fields.insert(name, value.to_string());
    } else {
      self.fields.insert(name, format!("{value:?}"));
    }
  }

  fn record_i64(&mut self, field: &Field, value: i64) {
    self.record_display(field, value);
  }

  fn record_u64(&mut self, field: &Field, value: u64) {
    self.record_display(field, value);
  }

  fn record_f64(&mut self, field: &Field, value: f64) {
    self.record_display(field, value);
  }

  fn record_bool(&mut self, field: &Field, value: bool) {
    self.record_display(field, value);
  }

  fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
    let name = field_name(field);
    if !name.starts_with("log.") {
      self.fields.insert(name, format!("{value:?}"));
    }
  }

  fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
    self.fields.insert(field_name(field), value.to_string());
  }
}

impl FieldVisitor {
  fn record_display<T: std::fmt::Display>(&mut self, field: &Field, value: T) {
    self.fields.insert(field_name(field), value.to_string());
  }
}

fn field_name(field: &Field) -> String {
  field.name().strip_prefix("r#").unwrap_or(field.name()).to_string()
}
