use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tracing::Dispatch;
use tracing::field::{Field, Visit};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt};

use crate::SESSION_LOG_TARGET;

static SESSION_LOG_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn with_session_log_subscriber<R>(
    log_dir: &Path,
    run: impl FnOnce() -> R,
) -> io::Result<R> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let parent = tracing::dispatcher::get_default(|dispatch| dispatch.clone());
    with_session_log_subscriber_with_filter(log_dir, filter, parent, run)
}

fn with_session_log_subscriber_with_filter<R>(
    log_dir: &Path,
    filter: EnvFilter,
    parent: Dispatch,
    run: impl FnOnce() -> R,
) -> io::Result<R> {
    fs::create_dir_all(log_dir)?;
    repair_log_newline_boundaries(log_dir, "session.log")?;
    let session_appender = tracing_appender::rolling::daily(log_dir, "session.log");
    let (session_writer, session_guard) = tracing_appender::non_blocking(session_appender);
    let session_layer = SessionJsonLayer::new(session_writer, parent);
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(session_layer);
    let result = tracing::subscriber::with_default(subscriber, run);
    drop(session_guard);
    Ok(result)
}

struct SessionJsonLayer<W> {
    writer: W,
    parent: Dispatch,
}

impl<W> SessionJsonLayer<W> {
    fn new(writer: W, parent: Dispatch) -> Self {
        Self { writer, parent }
    }
}

// `tracing_subscriber::fmt` cannot preserve `serde_json::Value` as a nested
// field here, so the session layer writes the small JSONL envelope directly.
impl<S, W> Layer<S> for SessionJsonLayer<W>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        if metadata.target() != SESSION_LOG_TARGET {
            // Keep normal service diagnostics flowing to the parent subscriber
            // while session threads use a thread-local subscriber.
            self.parent.event(event);
            return;
        }

        let mut visitor = SessionFieldVisitor::default();
        event.record(&mut visitor);
        visitor.fields.insert(
            "level".to_string(),
            SessionFieldValue::Json(Value::String(metadata.level().as_str().to_string())),
        );
        visitor.fields.insert(
            "target".to_string(),
            SessionFieldValue::Json(Value::String(metadata.target().to_string())),
        );
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs_f64())
            .unwrap_or_default();
        visitor.fields.insert(
            "timestamp".to_string(),
            SessionFieldValue::Json(json_number(timestamp)),
        );

        let mut line = Vec::new();
        if write_session_json_line(&mut line, &visitor.fields).is_ok() {
            line.push(b'\n');
            let _guard = SESSION_LOG_WRITE_LOCK.lock().ok();
            let mut writer = self.writer.make_writer();
            let _ = writer.write_all(&line);
        }
    }
}

#[derive(Default)]
struct SessionFieldVisitor {
    fields: BTreeMap<String, SessionFieldValue>,
}

enum SessionFieldValue {
    Json(Value),
    RawJson(String),
}

impl Visit for SessionFieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert_field(field, Value::String(value.to_string()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert_field(field, Value::Bool(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert_field(field, Value::Number(value.into()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert_field(field, Value::Number(value.into()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.insert_field(field, Value::String(format!("{value:?}")));
    }
}

impl SessionFieldVisitor {
    fn insert_field(&mut self, field: &Field, value: Value) {
        self.insert_named_field(field.name(), value);
    }

    fn insert_named_field(&mut self, name: &str, value: Value) {
        if name == "params_json" {
            let value = match value {
                Value::String(raw) => SessionFieldValue::RawJson(raw),
                other => SessionFieldValue::Json(other),
            };
            self.fields.insert("params".to_string(), value);
            return;
        }
        self.fields
            .insert(name.to_string(), SessionFieldValue::Json(value));
    }
}

fn write_session_json_line(
    writer: &mut impl Write,
    fields: &BTreeMap<String, SessionFieldValue>,
) -> io::Result<()> {
    writer.write_all(b"{")?;
    for (index, (key, value)) in fields.iter().enumerate() {
        if index > 0 {
            writer.write_all(b",")?;
        }
        serde_json::to_writer(&mut *writer, key)?;
        writer.write_all(b":")?;
        match value {
            SessionFieldValue::Json(value) => serde_json::to_writer(&mut *writer, value)?,
            SessionFieldValue::RawJson(raw) => writer.write_all(raw.as_bytes())?,
        }
    }
    writer.write_all(b"}")
}

fn json_number(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn repair_log_newline_boundaries(log_dir: &Path, prefix: &str) -> io::Result<()> {
    if !log_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(log_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with(prefix) {
            repair_file_newline_boundary(&path)?;
        }
    }
    Ok(())
}

fn repair_file_newline_boundary(path: &Path) -> io::Result<()> {
    let mut file = OpenOptions::new().read(true).append(true).open(path)?;
    if file.metadata()?.len() == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)?;
    if last[0] != b'\n' {
        file.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    #[test]
    fn session_subscriber_writes_session_events_with_structured_params() {
        let dir = tempfile::tempdir().unwrap();

        with_session_log_subscriber_with_filter(
            dir.path(),
            EnvFilter::new("info"),
            Dispatch::none(),
            || {
                tracing::info!(
                    target: SESSION_LOG_TARGET,
                    category = "session",
                    agent = "codex",
                    direction = "sent",
                    event = "turn/start",
                    params_json = r#"{"threadId":"thread-1","turn":{"id":"turn-1"}}"#,
                    rpc_id = "4",
                    "agent_session_message"
                );
                tracing::info!(
                    target: "vik_orchestrator::engine",
                    event = "service",
                    "service_event"
                );
            },
        )
        .unwrap();

        let body = session_log_body(dir.path());
        let lines = body.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 1);
        let value: Value = serde_json::from_str(lines[0]).unwrap();

        assert_eq!(value["category"], "session");
        assert_eq!(value["agent"], "codex");
        assert_eq!(value["direction"], "sent");
        assert_eq!(value["event"], "turn/start");
        assert_eq!(
            value["params"],
            json!({
                "threadId": "thread-1",
                "turn": { "id": "turn-1" }
            })
        );
        assert_eq!(value["rpc_id"], "4");
    }

    #[test]
    fn session_subscriber_forwards_service_events_to_parent_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let targets = Arc::new(Mutex::new(Vec::new()));
        let parent = Dispatch::new(RecordingSubscriber {
            targets: Arc::clone(&targets),
        });

        with_session_log_subscriber_with_filter(dir.path(), EnvFilter::new("info"), parent, || {
            tracing::info!(target: "vik_orchestrator::engine", "service_event");
        })
        .unwrap();

        assert_eq!(
            targets.lock().unwrap().as_slice(),
            ["vik_orchestrator::engine"]
        );
    }

    #[test]
    fn repair_log_newline_boundaries_separates_torn_session_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.log.2026-05-05");
        fs::write(&path, b"{\"partial\":true").unwrap();

        repair_log_newline_boundaries(dir.path(), "session.log").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"{\"partial\":true\n");
    }

    fn session_log_body(dir: &Path) -> String {
        let mut paths = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.starts_with("session.log"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        assert_eq!(paths.len(), 1);
        fs::read_to_string(&paths[0]).unwrap()
    }

    struct RecordingSubscriber {
        targets: Arc<Mutex<Vec<String>>>,
    }

    impl Subscriber for RecordingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            self.targets
                .lock()
                .unwrap()
                .push(event.metadata().target().to_string());
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }
}
