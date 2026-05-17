//! One stage execution = one session.
//!
//! [`Session`] is a cloneable handle around a spawned agent subprocess.
//! It owns the prompt-render → spawn → stream-events → terminate
//! pipeline and exposes:
//!
//! - [`Session::snapshot`] — point-in-time copy of state, tokens, rate
//!   limits (cheap, lock-released before return).
//! - [`Session::subscribe_state`] / [`Session::wait`] — `watch::Receiver`
//!   for state changes; the watch is the only synchronization with the
//!   internal `Mutex`-guarded snapshot.
//! - [`Session::cancel`] — kill the child and force `Cancelled` (no-op
//!   if already terminal).
//!
//! Provider events append to JSONL as the durable record. Decoded
//! semantic events also accumulate into the snapshot.
#![allow(dead_code)]
mod factory;
mod jsonl_writer;
mod snapshot;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use crate::agent::AgentEvent;
pub use factory::*;
pub use snapshot::*;

use chrono::Utc;
use thiserror::Error;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdout, Command};
use tokio::sync::watch;
use tracing::Instrument;

use crate::agent::{AgentAdapter, AgentCommand, AgentStdin, get_adapter};
use crate::config::AgentProfileSchema;
use crate::context::{Issue, IssueStage};
use crate::shell::{Child, CommandExecError, CommandExt};
use crate::template::{PromptRenderer, TemplateError};

use self::jsonl_writer::JsonlWriter;

/// Errors surfaced when building a session.
#[derive(Debug, Error)]
pub enum SessionError {
  #[error("unknown agent profile `{profile}`")]
  ProfileNotFound { profile: String },
  #[error(transparent)]
  AgentSpawn(#[from] CommandExecError),
  #[error(transparent)]
  TemplateRender(#[from] TemplateError),
  #[error("prompt path `{0}` could not be resolved")]
  PromptPath(PathBuf),
  #[error(transparent)]
  WriteLog(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct Session {
  stage: IssueStage,
  profile: AgentProfileSchema,
  agent: Arc<dyn AgentAdapter>,
  inner: Arc<Mutex<SessionInner>>,
  state_notifier: watch::Sender<SessionState>,
}

struct SessionInner {
  snapshot: SessionSnapshot,
  writer: Option<JsonlWriter>,
  child: Option<Child>,
}

impl Session {
  pub(super) async fn spawn(stage: IssueStage, profile: AgentProfileSchema) -> Result<Self, SessionError> {
    let now = Utc::now();

    let snapshot = SessionSnapshot {
      started_at: now,
      ..Default::default()
    };

    let agent = get_adapter(profile.runtime);

    let (state_notifier, _) = watch::channel(snapshot.state);

    let session = Self {
      stage,
      profile,
      agent,
      inner: Arc::new(Mutex::new(SessionInner {
        snapshot,
        writer: None,
        child: None,
      })),
      state_notifier,
    };

    let child = session.spawn_inner().await?;
    session.inner.lock().expect("session mutex never poisoned").child = Some(child);

    Ok(session)
  }

  pub fn id(&self) -> &str {
    &self.stage.issue().id
  }

  pub fn issue(&self) -> &Issue {
    self.stage.issue()
  }

  pub fn stage(&self) -> &IssueStage {
    &self.stage
  }

  pub fn snapshot(&self) -> SessionSnapshot {
    self.inner.lock().expect("session mutex never poisoned").snapshot.clone()
  }

  pub fn state(&self) -> SessionState {
    self.inner.lock().expect("session mutex never poisoned").snapshot.state
  }

  pub fn log_file(&self) -> &Path {
    self.stage.log_file()
  }

  pub fn terminated(&self) -> bool {
    self.state().is_terminated()
  }

  pub fn subscribe_state(&self) -> watch::Receiver<SessionState> {
    self.state_notifier.subscribe()
  }

  pub fn cancel(&self) {
    let mut inner = self.inner.lock().expect("session mutex never poisoned");
    if let Some(child) = &inner.child {
      tracing::info!("session cancelling");
      child.cancel();
      inner.set_state(SessionState::Cancelled, &self.state_notifier);
    }
  }

  pub async fn wait(&self) -> SessionSnapshot {
    let mut state_rx = self.state_notifier.subscribe();

    loop {
      if self.terminated() {
        return self.snapshot();
      }

      if state_rx.changed().await.is_err() {
        return self.snapshot();
      }
    }
  }

  async fn spawn_inner(&self) -> Result<Child, SessionError> {
    let agent_command = self.prepare().await?;

    self.set_state(SessionState::Running);

    let mut command = Command::new(&agent_command.program);
    command
      .current_dir(self.stage.workdir())
      .args(agent_command.args)
      .stdout(Stdio::piped())
      .stderr(Stdio::null());

    match &agent_command.stdin {
      AgentStdin::None => {
        command.stdin(Stdio::null());
      },
      AgentStdin::Inherit => {},
      AgentStdin::Pipe(_) => {
        command.stdin(Stdio::piped());
      },
    }

    let mut child = command
      .timeout(/* TODO: we need timeout here */ Duration::from_hours(1))
      .spawn()?;

    tracing::info!("session running");

    // For `Pipe`, write the prompt and drop the handle so the child
    // sees EOF — Codex blocks on stdin until that happens.
    if let AgentStdin::Pipe(input) = agent_command.stdin
      && let Some(mut stdin) = child.stdin.take()
    {
      stdin
        .write_all(input.as_bytes())
        .await
        .map_err(|err| SessionError::AgentSpawn(CommandExecError::Spawn(err)))?;
    }

    let stdout = child
      .stdout
      .take()
      .ok_or_else(|| std::io::Error::other("Stdout was not bound to spawned agent process"))?;

    self.stream_agent_output(stdout)?;

    Ok(child)
  }

  async fn prepare(&self) -> Result<AgentCommand, SessionError> {
    self.set_state(SessionState::Preparing);
    tracing::info!("session preparing");

    if let Some(parent) = self.log_file().parent() {
      fs::create_dir_all(parent).await?;
    }

    let prompt = self.render_prompt().await?;

    Ok(self.agent.build_command(&self.profile, prompt))
  }

  async fn render_prompt(&self) -> Result<String, SessionError> {
    let renderer = PromptRenderer::new();
    let prompt_file = self
      .stage
      .workflow()
      .resolve_path(&self.stage().stage().prompt_file)
      .ok_or_else(|| SessionError::PromptPath(self.stage().stage().prompt_file.clone()))?;

    let mut file = File::open(&prompt_file)
      .await
      .map_err(|err| SessionError::TemplateRender(TemplateError::Io(err)))?;

    let mut template = String::new();
    file
      .read_to_string(&mut template)
      .await
      .map_err(|err| SessionError::TemplateRender(TemplateError::Io(err)))?;

    Ok(renderer.render(&template, &self.stage).await?)
  }

  fn set_state(&self, state: SessionState) {
    self
      .inner
      .lock()
      .expect("session mutex never poisoned")
      .set_state(state, &self.state_notifier);
  }

  fn stream_agent_output(&self, stdout: ChildStdout) -> Result<(), SessionError> {
    let writer = JsonlWriter::open(self.log_file())?;

    self.inner.lock().expect("session mutex never poisoned").writer = Some(writer);

    let agent = self.agent.clone();
    let inner = Arc::clone(&self.inner);
    let state_notifier = self.state_notifier.clone();

    // Inherit the stage span so SessionStarted / terminal logs flatten
    // `phase`, `issue_id`, and `stage_name` onto each event.
    tokio::spawn(stream_agent_events(stdout, agent, inner, state_notifier).in_current_span());

    Ok(())
  }
}

impl SessionInner {
  fn finish_output_stream(&mut self, state_notifier: &watch::Sender<SessionState>) {
    if matches!(self.snapshot.state, SessionState::Running) {
      self.set_state(SessionState::Failed, state_notifier);
    }
  }

  fn apply_event(&mut self, event: AgentEvent, state_notifier: &watch::Sender<SessionState>) {
    let notify_state_watch = event.affects_snapshot();

    if let Some(writer) = &mut self.writer
      && let Err(err) = writer.write(&event)
    {
      tracing::error!("session jsonl write failed: {err}");
    }

    if notify_state_watch {
      self.snapshot.last_event_at = Some(Utc::now());
    }

    match event {
      AgentEvent::SessionStarted { session_id, .. } => {
        tracing::info!(session_id = %session_id, "agent session id observed");
        self.snapshot.agent_session_id = Some(session_id);
      },
      AgentEvent::Message { text, .. } => {
        self.snapshot.last_message = Some(text);
      },
      AgentEvent::TokenUsage {
        input,
        output,
        cache_read,
        ..
      } => {
        // saturating_add tolerates duplicated TokenUsage events
        // without panicking; providers occasionally report twice on
        // retry and the totals only grow.
        self.snapshot.tokens.input = self.snapshot.tokens.input.saturating_add(input);
        self.snapshot.tokens.output = self.snapshot.tokens.output.saturating_add(output);
        self.snapshot.tokens.cache_read = self.snapshot.tokens.cache_read.saturating_add(cache_read);
      },
      AgentEvent::RateLimit {
        scope,
        remaining,
        reset_at,
        observed_at,
        ..
      } => {
        // Latest-wins by observation time: a provider retry can land a
        // stale observation after a newer one, and we want to keep the
        // most recent ground truth.
        let keep = match self.snapshot.rate_limits.get(&scope) {
          Some(existing) => observed_at >= existing.observed_at,
          None => true,
        };
        if keep {
          self.snapshot.rate_limits.insert(
            scope,
            RateLimitObservation {
              remaining,
              reset_at,
              observed_at,
            },
          );
        }
      },
      AgentEvent::Completed { .. } => {
        self.set_state(SessionState::Completed, state_notifier);
      },
      AgentEvent::Error { detail: _, .. } => {
        self.set_state(SessionState::Failed, state_notifier);
      },
      AgentEvent::ToolCall { .. } | AgentEvent::Unknown { .. } => {},
    }

    if notify_state_watch {
      state_notifier.send_replace(self.snapshot.state);
    }
  }

  /// Terminal state is sticky: once Completed/Failed/Cancelled/Stalled
  /// is set, later transitions are ignored. Without this, the
  /// stdout-stream tail could overwrite a Completed with Failed when
  /// the child exits cleanly after its last event.
  fn set_state(&mut self, state: SessionState, state_notifier: &watch::Sender<SessionState>) {
    if self.snapshot.state.is_terminated() {
      return;
    }

    if state.is_terminated() {
      tracing::info!(state = ?state, "session terminal");
    }

    self.snapshot.state = state;
    state_notifier.send_replace(state);
  }
}

async fn stream_agent_events(
  stdout: ChildStdout,
  agent: Arc<dyn AgentAdapter>,
  inner: Arc<Mutex<SessionInner>>,
  state_notifier: watch::Sender<SessionState>,
) {
  let mut lines = BufReader::new(stdout).lines();

  loop {
    match lines.next_line().await {
      Ok(Some(line)) => {
        let events = map_agent_stdout_line(agent.as_ref(), &line);

        let mut inner = inner.lock().expect("session mutex never poisoned");
        for event in events {
          inner.apply_event(event, &state_notifier);
        }
      },
      Ok(None) => break,
      Err(err) => {
        inner.lock().expect("session mutex never poisoned").apply_event(
          AgentEvent::Error {
            detail: err.to_string(),
            raw: None,
          },
          &state_notifier,
        );
        break;
      },
    }
  }

  inner
    .lock()
    .expect("session mutex never poisoned")
    .finish_output_stream(&state_notifier);
}

fn map_agent_stdout_line(agent: &dyn AgentAdapter, line: &str) -> Vec<AgentEvent> {
  match agent.map_line(line) {
    Ok(events) => events,
    Err(err) => vec![AgentEvent::Error {
      detail: err.to_string(),
      raw: None,
    }],
  }
}
#[cfg(test)]
mod tests {
  use tracing::subscriber::with_default;
  use tracing_subscriber::{Registry, layer::SubscriberExt};

  use super::*;
  use crate::config::AgentRuntime;
  use crate::logging::tests::{CaptureLayer, captured_event, captured_message_exists};

  fn session_inner() -> (SessionInner, watch::Sender<SessionState>) {
    let snapshot = SessionSnapshot {
      started_at: Utc::now(),
      ..Default::default()
    };
    let (state_notifier, _) = watch::channel(snapshot.state);

    (
      SessionInner {
        snapshot,
        writer: None,
        child: None,
      },
      state_notifier,
    )
  }

  #[test]
  fn token_usage_events_accumulate_without_overflow() {
    let (mut inner, state_notifier) = session_inner();

    inner.apply_event(
      AgentEvent::TokenUsage {
        input: u64::MAX - 1,
        output: 10,
        cache_read: 20,
        raw: None,
      },
      &state_notifier,
    );
    inner.apply_event(
      AgentEvent::TokenUsage {
        input: 10,
        output: u64::MAX,
        cache_read: 30,
        raw: None,
      },
      &state_notifier,
    );

    assert_eq!(inner.snapshot.tokens.input, u64::MAX);
    assert_eq!(inner.snapshot.tokens.output, u64::MAX);
    assert_eq!(inner.snapshot.tokens.cache_read, 50);
  }

  #[test]
  fn rate_limit_observation_keeps_latest_event_per_scope() {
    let (mut inner, state_notifier) = session_inner();
    let reset_at = "2026-05-16T10:15:30Z".parse().expect("test timestamp parses");
    let stale = "2026-05-16T10:00:00Z".parse().expect("test timestamp parses");
    let fresh = "2026-05-16T10:05:00Z".parse().expect("test timestamp parses");

    inner.apply_event(
      AgentEvent::RateLimit {
        scope: "codex:tokens_per_min".into(),
        remaining: 50,
        reset_at,
        observed_at: fresh,
        raw: None,
      },
      &state_notifier,
    );
    inner.apply_event(
      AgentEvent::RateLimit {
        scope: "codex:tokens_per_min".into(),
        remaining: 10,
        reset_at,
        observed_at: stale,
        raw: None,
      },
      &state_notifier,
    );

    let observation = inner
      .snapshot
      .rate_limits
      .get("codex:tokens_per_min")
      .expect("rate limit observation stored");
    assert_eq!(observation.remaining, 50);
    assert_eq!(observation.observed_at, fresh);
  }

  #[test]
  fn codex_message_event_retains_raw_provider_line() {
    let agent = get_adapter(AgentRuntime::Codex);
    let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hello"}}"#;
    let raw = serde_json::from_str(line).expect("fixture is valid JSON");

    assert_eq!(
      map_agent_stdout_line(agent.as_ref(), line),
      vec![AgentEvent::Message {
        text: "hello".into(),
        raw: Some(raw),
      }]
    );
  }

  #[test]
  fn codex_tool_call_maps_to_neutral_tool_call() {
    let agent = get_adapter(AgentRuntime::Codex);
    let line = r#"{"type":"item.completed","item":{"id":"tool_0","type":"tool_call","name":"shell","arguments":"{}"}}"#;
    let raw = serde_json::from_str(line).expect("fixture is valid JSON");

    assert_eq!(
      map_agent_stdout_line(agent.as_ref(), line),
      vec![AgentEvent::ToolCall {
        id: Some("tool_0".into()),
        name: Some("shell".into()),
        input: Some(serde_json::json!("{}")),
        raw: Some(raw),
      }]
    );
  }

  #[test]
  fn codex_unknown_event_retains_raw_provider_line() {
    let agent = get_adapter(AgentRuntime::Codex);
    let line = r#"{"type":"future.event","payload":{"ok":true}}"#;
    let raw = serde_json::from_str(line).expect("fixture is valid JSON");

    assert_eq!(
      map_agent_stdout_line(agent.as_ref(), line),
      vec![AgentEvent::Unknown {
        event_type: Some("future.event".into()),
        raw,
      }]
    );
  }

  #[test]
  fn claude_tool_only_event_maps_to_neutral_tool_call() {
    let agent = get_adapter(AgentRuntime::ClaudeCode);
    let line =
      r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t-1","name":"Bash","input":{}}]}}"#;
    let raw = serde_json::from_str(line).expect("fixture is valid JSON");

    assert_eq!(
      map_agent_stdout_line(agent.as_ref(), line),
      vec![AgentEvent::ToolCall {
        id: Some("t-1".into()),
        name: Some("Bash".into()),
        input: Some(serde_json::json!({})),
        raw: Some(raw),
      }]
    );
  }

  #[test]
  fn claude_user_event_retains_raw_provider_line() {
    let agent = get_adapter(AgentRuntime::ClaudeCode);
    let line = r#"{"type":"user","message":{"content":[]}}"#;
    let raw = serde_json::from_str(line).expect("fixture is valid JSON");

    assert_eq!(
      map_agent_stdout_line(agent.as_ref(), line),
      vec![AgentEvent::Unknown {
        event_type: Some("user".into()),
        raw,
      }]
    );
  }

  #[test]
  fn invalid_jsonl_maps_to_error_without_raw_event() {
    let agent = get_adapter(AgentRuntime::Codex);
    let events = map_agent_stdout_line(agent.as_ref(), "{not-json");

    assert_eq!(events.len(), 1);
    assert!(matches!(events.first(), Some(AgentEvent::Error { .. })));
  }

  #[test]
  fn codex_fixture_retains_raw_for_each_provider_line() {
    let path = concat!(
      env!("CARGO_MANIFEST_DIR"),
      "/tests/fixtures/agent_events/codex/happy_path.jsonl"
    );
    assert_fixture_retains_raw_for_each_provider_line(AgentRuntime::Codex, path);
  }

  #[test]
  fn claude_code_fixture_retains_raw_for_each_provider_line() {
    let path = concat!(
      env!("CARGO_MANIFEST_DIR"),
      "/tests/fixtures/agent_events/claude_code/happy_path.jsonl"
    );
    assert_fixture_retains_raw_for_each_provider_line(AgentRuntime::ClaudeCode, path);
  }

  fn assert_fixture_retains_raw_for_each_provider_line(runtime: AgentRuntime, path: &str) {
    let agent = get_adapter(runtime);
    let body = std::fs::read_to_string(path).expect("fixture present");

    for (index, line) in body.lines().enumerate() {
      let events = map_agent_stdout_line(agent.as_ref(), line);
      let raw: serde_json::Value = serde_json::from_str(line).expect("fixture line is JSON");
      assert!(!events.is_empty(), "line {index} must produce at least one event");
      assert!(
        events.iter().all(|event| event.raw() == Some(&raw)),
        "line {index} must retain raw provider JSON on every emitted event"
      );
    }
  }

  #[test]
  fn raw_only_events_do_not_change_snapshot_semantics() {
    let (state_notifier, state_rx) = watch::channel(SessionState::Running);
    let mut inner = SessionInner {
      snapshot: SessionSnapshot {
        state: SessionState::Running,
        ..Default::default()
      },
      writer: None,
      child: None,
    };

    inner.apply_event(
      AgentEvent::Unknown {
        event_type: Some("future.event".into()),
        raw: serde_json::json!({"type":"future.event"}),
      },
      &state_notifier,
    );
    inner.apply_event(
      AgentEvent::ToolCall {
        id: Some("tool_0".into()),
        name: Some("shell".into()),
        input: Some(serde_json::json!("{}")),
        raw: Some(serde_json::json!({"type":"item.completed"})),
      },
      &state_notifier,
    );

    assert!(matches!(inner.snapshot.state, SessionState::Running));
    assert!(inner.snapshot.last_message.is_none());
    assert_eq!(inner.snapshot.tokens.input, 0);
    assert_eq!(inner.snapshot.tokens.output, 0);
    assert_eq!(inner.snapshot.tokens.cache_read, 0);
    assert!(inner.snapshot.last_event_at.is_none());
    assert!(!state_rx.has_changed().expect("state channel open"));
  }

  #[test]
  fn jsonl_writer_persists_raw_and_semantic_events_in_order() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let log_file = tempdir.path().join("session.jsonl");
    let raw_event = AgentEvent::ToolCall {
      id: Some("tool_0".into()),
      name: Some("shell".into()),
      input: None,
      raw: Some(serde_json::json!({
        "type": "item.completed",
        "item": {
          "id": "tool_0",
          "type": "tool_call",
          "name": "shell"
        }
      })),
    };
    let message_event = AgentEvent::Message {
      text: "hello".into(),
      raw: None,
    };

    {
      let mut writer = JsonlWriter::open(&log_file).expect("open JSONL writer");
      writer.write(&raw_event).expect("write raw event");
      writer.write(&message_event).expect("write message event");
    }

    let contents = std::fs::read_to_string(&log_file).expect("read JSONL");
    let events: Vec<AgentEvent> = contents
      .lines()
      .map(|line| serde_json::from_str(line).expect("event JSON"))
      .collect();

    assert_eq!(events, vec![raw_event, message_event]);
  }

  #[test]
  fn set_state_emits_terminal_log_on_terminal_transition() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    with_default(subscriber, || {
      let (mut inner, state_notifier) = session_inner();
      inner.set_state(SessionState::Running, &state_notifier);
      inner.set_state(SessionState::Completed, &state_notifier);
    });

    let events = events.lock().expect("events mutex");
    assert!(captured_message_exists(&events, "session terminal"));
    let event = captured_event(&events, "session terminal");
    assert_eq!(event["state"], "Completed");
  }

  #[test]
  fn apply_event_emits_agent_session_id_log() {
    let (layer, events) = CaptureLayer::new();
    let subscriber = Registry::default().with(layer);

    with_default(subscriber, || {
      let (mut inner, state_notifier) = session_inner();
      inner.apply_event(
        AgentEvent::SessionStarted {
          session_id: "sess-123".into(),
          raw: None,
        },
        &state_notifier,
      );
    });

    let events = events.lock().expect("events mutex");
    let event = captured_event(&events, "agent session id observed");
    assert_eq!(event["session_id"], "sess-123");
  }
}
