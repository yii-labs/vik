//! Codex provider adapter.
//!
//! Command shape:
//!
//! ```text
//! codex exec [profile.args...] --json -m <model>
//! ```
//!
//! Prompt is written to stdin; Codex blocks until EOF.
//!
//! Codex emits two event shapes today and we accept both: the legacy
//! `{"msg": {"type": ...}}` envelope and the newer flat
//! `{"type": "thread.started" | "item.completed" | "turn.completed"}`.
//! Unknown shapes have no semantic snapshot effect, but the session
//! still writes them as provider-neutral `unknown` records with raw
//! JSON.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::{AgentAdapter, AgentCommand, AgentEvent, AgentStdin, build_extra_args};
use crate::config::AgentProfileSchema;

const CODEX_PROGRAM: &str = "codex";

#[derive(Debug, Clone)]
pub struct CodexAdapter;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodexEvent {
  #[serde(flatten)]
  pub kind: CodexEventKind,
  pub raw: Value,
}

impl<'de> Deserialize<'de> for CodexEvent {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: Deserializer<'de>,
  {
    let value = Value::deserialize(deserializer)?;
    if value.get("raw").is_some() && value.get("kind").is_some() {
      #[derive(Deserialize)]
      struct StoredCodexEvent {
        #[serde(flatten)]
        kind: CodexEventKind,
        raw: Value,
      }

      if let Ok(stored) = serde_json::from_value::<StoredCodexEvent>(value.clone()) {
        return Ok(Self {
          kind: stored.kind,
          raw: stored.raw,
        });
      }
    }

    Ok(Self::from_provider_value(value))
  }
}

impl CodexEvent {
  fn from_provider_value(raw: Value) -> Self {
    let kind = codex_event_kind(&raw);
    Self { kind, raw }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodexEventKind {
  SessionConfigured {
    event_id: Option<String>,
    session_id: Option<String>,
  },
  AgentMessage {
    text: Option<String>,
  },
  TokenCount {
    usage: Option<CodexUsage>,
  },
  RateLimitWarning {
    scope: Option<String>,
    remaining: Option<u64>,
    reset_at: Option<String>,
  },
  RateLimitReset {
    scope: Option<String>,
    remaining: Option<u64>,
    reset_at: Option<String>,
  },
  TurnComplete,
  ShutdownComplete,
  ThreadStarted {
    thread_id: Option<String>,
  },
  ItemCompleted {
    item_type: Option<String>,
    item: Option<Value>,
  },
  TurnCompleted {
    usage: Option<CodexUsage>,
  },
  Error {
    message: Option<String>,
  },
  Unknown {
    event_type: Option<String>,
  },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexUsage {
  pub input_tokens: u64,
  pub output_tokens: u64,
  pub cached_input_tokens: u64,
}

impl CodexUsage {
  fn from_value(value: &Value) -> Self {
    Self {
      input_tokens: value.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
      output_tokens: value.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
      cached_input_tokens: value.get("cached_input_tokens").and_then(Value::as_u64).unwrap_or(0),
    }
  }
}

impl AgentAdapter for CodexAdapter {
  fn build_command(&self, profile: &AgentProfileSchema, prompt: String) -> AgentCommand {
    let mut args: Vec<String> = vec!["exec".into()];

    args.extend(build_extra_args(&profile.args));

    args.extend(["--json".into(), "-m".into(), profile.model.clone()]);

    AgentCommand {
      program: CODEX_PROGRAM.into(),
      args,
      stdin: AgentStdin::Pipe(prompt),
    }
  }

  fn map_line(&self, line: &str) -> Result<Vec<AgentEvent>, serde_json::Error> {
    let event: CodexEvent = serde_json::from_str(line)?;
    Ok(map_events(&event))
  }
}

fn codex_event_kind(value: &Value) -> CodexEventKind {
  if let Some(msg_ty) = value.get("msg").and_then(|msg| msg.get("type")).and_then(Value::as_str) {
    return match msg_ty {
      "session_configured" => CodexEventKind::SessionConfigured {
        event_id: value.get("id").and_then(Value::as_str).map(ToString::to_string),
        session_id: value
          .get("msg")
          .and_then(|msg| msg.get("session_id"))
          .and_then(Value::as_str)
          .or_else(|| value.get("id").and_then(Value::as_str))
          .map(ToString::to_string),
      },
      "agent_message" => CodexEventKind::AgentMessage {
        text: value
          .get("msg")
          .and_then(|msg| msg.get("message"))
          .and_then(Value::as_str)
          .or_else(|| value.get("msg").and_then(|msg| msg.get("text")).and_then(Value::as_str))
          .map(ToString::to_string),
      },
      "token_count" => CodexEventKind::TokenCount {
        usage: codex_legacy_usage(value),
      },
      "rate_limit_warning" => codex_rate_limit(value, true),
      "rate_limit_reset" => codex_rate_limit(value, false),
      "turn_complete" => CodexEventKind::TurnComplete,
      "shutdown_complete" => CodexEventKind::ShutdownComplete,
      "error" => CodexEventKind::Error {
        message: value
          .get("msg")
          .and_then(|msg| msg.get("message"))
          .and_then(Value::as_str)
          .map(ToString::to_string),
      },
      other => CodexEventKind::Unknown {
        event_type: Some(other.to_string()),
      },
    };
  }

  let Some(ty) = value.get("type").and_then(Value::as_str) else {
    return CodexEventKind::Unknown { event_type: None };
  };

  match ty {
    "thread.started" => CodexEventKind::ThreadStarted {
      thread_id: value.get("thread_id").and_then(Value::as_str).map(ToString::to_string),
    },
    "item.completed" => CodexEventKind::ItemCompleted {
      item_type: value
        .get("item")
        .and_then(|item| item.get("type"))
        .and_then(Value::as_str)
        .map(ToString::to_string),
      item: value.get("item").cloned(),
    },
    "turn.completed" => CodexEventKind::TurnCompleted {
      usage: value.get("usage").map(CodexUsage::from_value),
    },
    "error" => CodexEventKind::Error {
      message: value.get("message").and_then(Value::as_str).map(ToString::to_string),
    },
    other => CodexEventKind::Unknown {
      event_type: Some(other.to_string()),
    },
  }
}

fn codex_legacy_usage(value: &Value) -> Option<CodexUsage> {
  let info = value.get("msg")?.get("info")?;
  let totals = info.get("total_token_usage").unwrap_or(info);
  Some(CodexUsage::from_value(totals))
}

fn codex_rate_limit(value: &Value, warning: bool) -> CodexEventKind {
  let msg = value.get("msg");
  let scope = msg
    .and_then(|msg| msg.get("scope"))
    .and_then(Value::as_str)
    .map(ToString::to_string);
  let remaining = msg.and_then(|msg| msg.get("remaining")).and_then(Value::as_u64);
  let reset_at = msg
    .and_then(|msg| msg.get("reset_at"))
    .and_then(Value::as_str)
    .map(ToString::to_string);

  if warning {
    CodexEventKind::RateLimitWarning {
      scope,
      remaining,
      reset_at,
    }
  } else {
    CodexEventKind::RateLimitReset {
      scope,
      remaining,
      reset_at,
    }
  }
}

fn map_events(event: &CodexEvent) -> Vec<AgentEvent> {
  match &event.kind {
    CodexEventKind::SessionConfigured { session_id, .. } => session_id
      .as_ref()
      .map(|session_id| {
        vec![AgentEvent::SessionStarted {
          session_id: session_id.clone(),
          raw: Some(event.raw.clone()),
        }]
      })
      .unwrap_or_else(|| vec![unknown_event("session_configured", event)]),
    CodexEventKind::AgentMessage { text } => vec![AgentEvent::Message {
      text: text.clone().unwrap_or_default(),
      raw: Some(event.raw.clone()),
    }],
    CodexEventKind::TokenCount { usage } => usage
      .as_ref()
      .map(|usage| vec![token_usage_event(usage, Some(event.raw.clone()))])
      .unwrap_or_else(|| vec![unknown_event("token_count", event)]),
    CodexEventKind::RateLimitWarning {
      scope,
      remaining,
      reset_at,
    }
    | CodexEventKind::RateLimitReset {
      scope,
      remaining,
      reset_at,
    } => {
      vec![rate_limit_event(scope, *remaining, reset_at, Some(event.raw.clone()))]
    },
    CodexEventKind::TurnComplete | CodexEventKind::ShutdownComplete => {
      vec![AgentEvent::Completed {
        raw: Some(event.raw.clone()),
      }]
    },
    CodexEventKind::ThreadStarted { thread_id } => thread_id
      .as_ref()
      .map(|session_id| {
        vec![AgentEvent::SessionStarted {
          session_id: session_id.clone(),
          raw: Some(event.raw.clone()),
        }]
      })
      .unwrap_or_else(|| vec![unknown_event("thread.started", event)]),
    CodexEventKind::ItemCompleted { item_type, item } => match item_type.as_deref() {
      Some("agent_message") => vec![AgentEvent::Message {
        text: item
          .as_ref()
          .and_then(|item| item.get("text"))
          .and_then(Value::as_str)
          .unwrap_or("")
          .to_string(),
        raw: Some(event.raw.clone()),
      }],
      Some("tool_call") => vec![tool_call_event(item.as_ref(), Some(event.raw.clone()))],
      Some(other) => vec![AgentEvent::Unknown {
        event_type: Some(format!("item.completed:{other}")),
        raw: event.raw.clone(),
      }],
      None => vec![unknown_event("item.completed", event)],
    },
    CodexEventKind::TurnCompleted { usage } => {
      let mut events = Vec::new();
      if let Some(usage) = usage {
        events.push(token_usage_event(usage, Some(event.raw.clone())));
      }
      events.push(AgentEvent::Completed {
        raw: Some(event.raw.clone()),
      });
      events
    },
    CodexEventKind::Error { message } => vec![AgentEvent::Error {
      detail: message.clone().unwrap_or_default(),
      raw: Some(event.raw.clone()),
    }],
    CodexEventKind::Unknown { event_type } => {
      tracing::debug!(
        runtime = "codex",
        codex_event_type = event_type.as_deref().unwrap_or("unknown"),
        "codex event ignored: unknown type",
      );
      vec![AgentEvent::Unknown {
        event_type: event_type.clone(),
        raw: event.raw.clone(),
      }]
    },
  }
}

fn token_usage_event(usage: &CodexUsage, raw: Option<Value>) -> AgentEvent {
  AgentEvent::TokenUsage {
    input: usage.input_tokens,
    output: usage.output_tokens,
    cache_read: usage.cached_input_tokens,
    raw,
  }
}

fn rate_limit_event(
  scope: &Option<String>,
  remaining: Option<u64>,
  reset_at: &Option<String>,
  raw: Option<Value>,
) -> AgentEvent {
  let reset_at = reset_at
    .as_deref()
    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    .map(|dt| dt.with_timezone(&chrono::Utc))
    .unwrap_or_else(chrono::Utc::now);
  AgentEvent::RateLimit {
    scope: format!("codex:{}", scope.as_deref().unwrap_or("unknown")),
    remaining: remaining.unwrap_or(0),
    reset_at,
    observed_at: chrono::Utc::now(),
    raw,
  }
}

fn tool_call_event(item: Option<&Value>, raw: Option<Value>) -> AgentEvent {
  let id = item
    .and_then(|item| item.get("id").or_else(|| item.get("call_id")))
    .and_then(Value::as_str)
    .map(ToString::to_string);
  let name = item
    .and_then(|item| item.get("name").or_else(|| item.get("tool_name")))
    .and_then(Value::as_str)
    .map(ToString::to_string);
  let input = item
    .and_then(|item| item.get("input").or_else(|| item.get("arguments")))
    .cloned();

  AgentEvent::ToolCall { id, name, input, raw }
}

fn unknown_event(event_type: &str, event: &CodexEvent) -> AgentEvent {
  AgentEvent::Unknown {
    event_type: Some(event_type.to_string()),
    raw: event.raw.clone(),
  }
}

#[cfg(test)]
mod tests {
  use crate::config::AgentRuntime;

  use super::*;

  fn parse_events(line: &str) -> Vec<AgentEvent> {
    let event: CodexEvent = serde_json::from_str(line).expect("fixture is valid Codex event");
    map_events(&event)
  }

  fn parse_one(line: &str) -> Option<AgentEvent> {
    parse_events(line).into_iter().next()
  }

  fn raw(line: &str) -> serde_json::Value {
    serde_json::from_str(line).expect("fixture is valid JSON")
  }

  #[test]
  fn provider_event_kind_recognizes_known_legacy_and_flat_types() {
    let legacy: CodexEvent =
      serde_json::from_str(r#"{"id":"evt-2","msg":{"type":"token_count","info":{}}}"#).expect("fixture");
    assert_eq!(
      legacy.kind,
      CodexEventKind::TokenCount {
        usage: Some(CodexUsage {
          input_tokens: 0,
          output_tokens: 0,
          cached_input_tokens: 0,
        }),
      }
    );

    let flat: CodexEvent =
      serde_json::from_str(r#"{"type":"item.completed","item":{"id":"tool_0","type":"tool_call"}}"#).expect("fixture");
    assert_eq!(
      flat.kind,
      CodexEventKind::ItemCompleted {
        item_type: Some("tool_call".into()),
        item: Some(serde_json::json!({
          "id": "tool_0",
          "type": "tool_call",
        })),
      }
    );
  }

  #[test]
  fn session_configured_maps_to_session_started() {
    let line = r#"{"id":"evt-0","msg":{"type":"session_configured","session_id":"S-1"}}"#;
    assert_eq!(
      parse_one(line),
      Some(AgentEvent::SessionStarted {
        session_id: "S-1".into(),
        raw: Some(raw(line)),
      })
    );
  }

  #[test]
  fn agent_message_maps_to_message() {
    let line = r#"{"id":"evt-1","msg":{"type":"agent_message","message":"hi"}}"#;
    assert_eq!(
      parse_one(line),
      Some(AgentEvent::Message {
        text: "hi".into(),
        raw: Some(raw(line)),
      })
    );
  }

  #[test]
  fn token_count_reads_total_usage() {
    let line = r#"{"id":"evt-2","msg":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":20,"cached_input_tokens":5}}}}"#;
    assert_eq!(
      parse_one(line),
      Some(AgentEvent::TokenUsage {
        input: 10,
        output: 20,
        cache_read: 5,
        raw: Some(raw(line)),
      })
    );
  }

  #[test]
  fn turn_complete_maps_to_completed() {
    let line = r#"{"id":"evt-3","msg":{"type":"turn_complete"}}"#;
    assert_eq!(parse_one(line), Some(AgentEvent::Completed { raw: Some(raw(line)) }));
  }

  #[test]
  fn unknown_event_retains_raw_provider_line() {
    let line = r#"{"id":"evt-4","msg":{"type":"future_event_kind"}}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::Unknown {
        event_type: Some("future_event_kind".into()),
        raw: raw(line),
      }]
    );
  }

  #[test]
  fn rate_limit_emits_scope_prefix() {
    let line = r#"{"id":"evt-5","msg":{"type":"rate_limit_warning","scope":"tokens_per_min","remaining":100}}"#;
    match parse_one(line) {
      Some(AgentEvent::RateLimit { scope, remaining, .. }) => {
        assert_eq!(scope, "codex:tokens_per_min");
        assert_eq!(remaining, 100);
      },
      other => panic!("expected RateLimit, got {other:?}"),
    }
  }

  #[test]
  fn error_event_maps_to_provider_error() {
    let line = r#"{"id":"evt-6","msg":{"type":"error","message":"boom"}}"#;
    match parse_one(line) {
      Some(AgentEvent::Error { detail, raw: event_raw }) => {
        assert_eq!(detail, "boom");
        assert_eq!(event_raw, Some(raw(line)));
      },
      other => panic!("expected Error, got {other:?}"),
    }
  }

  #[test]
  fn current_thread_started_maps_to_session_started() {
    let line = r#"{"type":"thread.started","thread_id":"T-1"}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::SessionStarted {
        session_id: "T-1".into(),
        raw: Some(raw(line)),
      }]
    );
  }

  #[test]
  fn current_item_completed_agent_message_maps_to_message() {
    let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hello"}}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::Message {
        text: "hello".into(),
        raw: Some(raw(line)),
      }]
    );
  }

  #[test]
  fn current_item_completed_tool_call_maps_to_tool_call() {
    let line = r#"{"type":"item.completed","item":{"id":"tool_0","type":"tool_call","name":"shell","arguments":"{}"}}"#;
    assert_eq!(
      parse_events(line),
      vec![AgentEvent::ToolCall {
        id: Some("tool_0".into()),
        name: Some("shell".into()),
        input: Some(serde_json::json!("{}")),
        raw: Some(raw(line)),
      }]
    );
  }

  #[test]
  fn current_turn_completed_maps_usage_and_completed() {
    let line = r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":2}}"#;
    assert_eq!(
      parse_events(line),
      vec![
        AgentEvent::TokenUsage {
          input: 10,
          output: 2,
          cache_read: 4,
          raw: Some(raw(line)),
        },
        AgentEvent::Completed { raw: Some(raw(line)) },
      ]
    );
  }

  #[test]
  fn happy_path_fixture_maps_to_expected_sequence() {
    let path = concat!(
      env!("CARGO_MANIFEST_DIR"),
      "/tests/fixtures/agent_events/codex/happy_path.jsonl"
    );
    let body = std::fs::read_to_string(path).expect("fixture present");
    let mut events: Vec<AgentEvent> = Vec::new();
    for line in body.lines() {
      events.extend(parse_events(line));
    }
    assert!(
      matches!(events[0], AgentEvent::SessionStarted { .. }),
      "first event must be SessionStarted, got {:?}",
      events.first()
    );
    assert_eq!(
      events.iter().filter(|e| matches!(e, AgentEvent::Message { .. })).count(),
      2,
      "fixture must contribute two assistant messages"
    );
    assert!(
      events.iter().any(|e| matches!(e, AgentEvent::TokenUsage { .. })),
      "fixture must contribute a TokenUsage"
    );
    assert!(
      events.iter().filter(|e| matches!(e, AgentEvent::Completed { .. })).count() == 1,
      "turn.completed yields one Completed"
    );
  }

  #[test]
  fn rate_limit_fixture_yields_rate_limit_and_error() {
    let path = concat!(
      env!("CARGO_MANIFEST_DIR"),
      "/tests/fixtures/agent_events/codex/rate_limit_and_error.jsonl"
    );
    let body = std::fs::read_to_string(path).expect("fixture present");
    let mut rl_count = 0usize;
    let mut saw_error = false;
    for line in body.lines() {
      match parse_one(line) {
        Some(AgentEvent::RateLimit { scope, .. }) => {
          assert!(scope.starts_with("codex:"), "scope must be prefixed");
          rl_count += 1;
        },
        Some(AgentEvent::Error { .. }) => saw_error = true,
        _ => {},
      }
    }
    assert_eq!(rl_count, 2, "warning + reset each yield RateLimit");
    assert!(saw_error, "fixture includes one provider error line");
  }

  #[test]
  fn command_contains_expected_flags_and_stdin_pipe() {
    let adapter = CodexAdapter;
    let req =
      AgentProfileSchema::new(AgentRuntime::Codex, "gpt-5.5".into()).with_args(serde_yaml::Mapping::from_iter([
        (
          serde_yaml::Value::String("--config".into()),
          serde_yaml::Value::Sequence(serde_yaml::Sequence::from_iter(["model_reasoning_effort=high".into()])),
        ),
        (
          serde_yaml::Value::String("--ephemeral".into()),
          serde_yaml::Value::Bool(true),
        ),
        (
          serde_yaml::Value::String("--ignore-rules".into()),
          serde_yaml::Value::Bool(false),
        ),
      ]));

    let cmd = adapter.build_command(&req, "hello".into());

    assert_eq!(cmd.program, "codex");
    assert!(cmd.args.contains(&"--json".to_string()));
    assert!(cmd.args.contains(&"-m".to_string()));
    assert!(cmd.args.contains(&"gpt-5.5".to_string()));
    assert!(
      cmd.args.iter().any(|a| a == "model_reasoning_effort=high"),
      "typed param forwarded as --config override"
    );
    assert!(
      cmd.args.iter().any(|a| a == "--ephemeral"),
      "true boolean args are forwarded as no-value flags"
    );
    assert!(
      !cmd.args.iter().any(|a| a == "--ignore-rules"),
      "false boolean args are omitted"
    );
    match cmd.stdin {
      AgentStdin::Pipe(payload) => assert_eq!(payload, "hello"),
      other => panic!("expected Pipe(prompt), got {other:?}"),
    }
  }
}
