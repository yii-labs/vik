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
//! still writes them as typed unknown Codex provider records.

use serde_json::Value;

use super::{AgentAdapter, AgentCommand, AgentEvent, AgentStdin, build_extra_args};
use crate::agent::CodexProviderEventKind;
use crate::config::AgentProfileSchema;

const CODEX_PROGRAM: &str = "codex";

#[derive(Debug, Clone)]
pub struct CodexAdapter;

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

  fn provider_event(&self, value: Value) -> AgentEvent {
    AgentEvent::CodexProviderEvent {
      event_type: provider_event_kind(&value),
      event: value,
    }
  }

  fn map_event(&self, value: &Value) -> Vec<AgentEvent> {
    map_events(value)
  }
}

fn provider_event_kind(value: &Value) -> CodexProviderEventKind {
  if let Some(msg_ty) = value.get("msg").and_then(|msg| msg.get("type")).and_then(Value::as_str) {
    return match msg_ty {
      "session_configured" => CodexProviderEventKind::SessionConfigured,
      "agent_message" => CodexProviderEventKind::AgentMessage,
      "token_count" => CodexProviderEventKind::TokenCount,
      "rate_limit_warning" => CodexProviderEventKind::RateLimitWarning,
      "rate_limit_reset" => CodexProviderEventKind::RateLimitReset,
      "turn_complete" => CodexProviderEventKind::TurnComplete,
      "shutdown_complete" => CodexProviderEventKind::ShutdownComplete,
      "error" => CodexProviderEventKind::Error,
      other => CodexProviderEventKind::Unknown {
        event_type: Some(other.to_string()),
      },
    };
  }

  let Some(ty) = value.get("type").and_then(Value::as_str) else {
    return CodexProviderEventKind::Unknown { event_type: None };
  };

  match ty {
    "thread.started" => CodexProviderEventKind::ThreadStarted,
    "item.completed" => CodexProviderEventKind::ItemCompleted {
      item_type: value
        .get("item")
        .and_then(|item| item.get("type"))
        .and_then(Value::as_str)
        .map(ToString::to_string),
    },
    "turn.completed" => CodexProviderEventKind::TurnCompleted,
    "error" => CodexProviderEventKind::Error,
    other => CodexProviderEventKind::Unknown {
      event_type: Some(other.to_string()),
    },
  }
}

fn map_events(value: &Value) -> Vec<AgentEvent> {
  // Try the legacy nested-`msg` form first; if that misses, fall through
  // to the flat `type` form. Trying both lets us cope with mixed
  // streams from different Codex CLI versions in one run.
  if let Some(event) = map_value(value) {
    return vec![event];
  }

  let Some(ty) = value.get("type").and_then(Value::as_str) else {
    return Vec::new();
  };

  match ty {
    "thread.started" => value
      .get("thread_id")
      .and_then(Value::as_str)
      .map(|session_id| {
        vec![AgentEvent::SessionStarted {
          session_id: session_id.to_string(),
        }]
      })
      .unwrap_or_default(),
    "item.completed" => map_current_item_completed(value),
    "turn.completed" => map_current_turn_completed(value),
    "error" => vec![AgentEvent::Error {
      detail: value.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
    }],
    other => {
      tracing::debug!(
        runtime = "codex",
        codex_event_type = other,
        "codex event ignored: unknown type",
      );
      Vec::new()
    },
  }
}

fn map_current_item_completed(value: &Value) -> Vec<AgentEvent> {
  let Some(item) = value.get("item") else {
    return Vec::new();
  };
  let Some(item_type) = item.get("type").and_then(Value::as_str) else {
    return Vec::new();
  };

  match item_type {
    "agent_message" => vec![AgentEvent::Message {
      text: item.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
    }],
    _ => Vec::new(),
  }
}

/// `turn.completed` carries both the per-turn usage and the stream
/// terminator — fan out into two events so the session sees both.
fn map_current_turn_completed(value: &Value) -> Vec<AgentEvent> {
  let mut events = Vec::new();
  if let Some(usage) = value.get("usage") {
    events.push(AgentEvent::TokenUsage {
      input: usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
      output: usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
      cache_read: usage.get("cached_input_tokens").and_then(Value::as_u64).unwrap_or(0),
    });
  }
  events.push(AgentEvent::Completed);
  events
}

/// `pub(super)` so the in-module tests below can drive it directly
/// without round-tripping through the adapter.
pub(super) fn map_value(value: &Value) -> Option<AgentEvent> {
  let msg = value.get("msg")?;
  let ty = msg.get("type")?.as_str()?;

  match ty {
    "session_configured" => {
      // Newer codex versions report session id under `msg.session_id`,
      // older ones under the outer envelope's `id`. Try both.
      let session_id = msg
        .get("session_id")
        .and_then(Value::as_str)
        .or_else(|| value.get("id").and_then(Value::as_str))?
        .to_string();
      Some(AgentEvent::SessionStarted { session_id })
    },
    "agent_message" => {
      let text = msg
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| msg.get("text").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
      Some(AgentEvent::Message { text })
    },
    "token_count" => {
      let info = msg.get("info")?;
      // Prefer the `total_token_usage` subtree when present — it
      // already represents the cumulative count, so we don't need to
      // accumulate per-turn deltas ourselves.
      let totals = info.get("total_token_usage").unwrap_or(info);
      let input = totals.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
      let output = totals.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
      let cache_read = totals.get("cached_input_tokens").and_then(Value::as_u64).unwrap_or(0);
      Some(AgentEvent::TokenUsage {
        input,
        output,
        cache_read,
      })
    },
    "rate_limit_warning" | "rate_limit_reset" => {
      let scope_raw = msg.get("scope").and_then(Value::as_str).unwrap_or("unknown");
      // Provider prefix keeps Codex limits from colliding with Claude
      // limits in the session's per-scope map.
      let scope = format!("codex:{scope_raw}");
      let remaining = msg.get("remaining").and_then(Value::as_u64).unwrap_or(0);
      let reset_at = msg
        .get("reset_at")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
      Some(AgentEvent::RateLimit {
        scope,
        remaining,
        reset_at,
        observed_at: chrono::Utc::now(),
      })
    },
    "turn_complete" | "shutdown_complete" => Some(AgentEvent::Completed),
    "error" => {
      let detail = msg.get("message").and_then(Value::as_str).unwrap_or("").to_string();
      Some(AgentEvent::Error { detail })
    },
    other => {
      tracing::debug!(
        runtime = "codex",
        codex_event_type = other,
        "codex event ignored: unknown type",
      );
      None
    },
  }
}

#[cfg(test)]
mod tests {
  use crate::config::AgentRuntime;

  use super::*;

  fn parse(line: &str) -> Option<AgentEvent> {
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");
    map_value(&value)
  }

  fn parse_events(line: &str) -> Vec<AgentEvent> {
    let value: Value = serde_json::from_str(line).expect("fixture is valid JSON");
    map_events(&value)
  }

  #[test]
  fn provider_event_kind_recognizes_known_legacy_and_flat_types() {
    let legacy: Value =
      serde_json::from_str(r#"{"id":"evt-2","msg":{"type":"token_count","info":{}}}"#).expect("fixture");
    assert_eq!(provider_event_kind(&legacy), CodexProviderEventKind::TokenCount);

    let flat: Value =
      serde_json::from_str(r#"{"type":"item.completed","item":{"id":"tool_0","type":"tool_call"}}"#).expect("fixture");
    assert_eq!(
      provider_event_kind(&flat),
      CodexProviderEventKind::ItemCompleted {
        item_type: Some("tool_call".into()),
      }
    );
  }

  #[test]
  fn session_configured_maps_to_session_started() {
    let line = r#"{"id":"evt-0","msg":{"type":"session_configured","session_id":"S-1"}}"#;
    assert_eq!(
      parse(line),
      Some(AgentEvent::SessionStarted {
        session_id: "S-1".into(),
      })
    );
  }

  #[test]
  fn agent_message_maps_to_message() {
    let line = r#"{"id":"evt-1","msg":{"type":"agent_message","message":"hi"}}"#;
    assert_eq!(parse(line), Some(AgentEvent::Message { text: "hi".into() }));
  }

  #[test]
  fn token_count_reads_total_usage() {
    let line = r#"{"id":"evt-2","msg":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":20,"cached_input_tokens":5}}}}"#;
    assert_eq!(
      parse(line),
      Some(AgentEvent::TokenUsage {
        input: 10,
        output: 20,
        cache_read: 5,
      })
    );
  }

  #[test]
  fn turn_complete_maps_to_completed() {
    let line = r#"{"id":"evt-3","msg":{"type":"turn_complete"}}"#;
    assert_eq!(parse(line), Some(AgentEvent::Completed));
  }

  #[test]
  fn unknown_event_has_no_semantic_mapping() {
    let line = r#"{"id":"evt-4","msg":{"type":"future_event_kind"}}"#;
    assert_eq!(parse(line), None);
  }

  #[test]
  fn rate_limit_emits_scope_prefix() {
    let line = r#"{"id":"evt-5","msg":{"type":"rate_limit_warning","scope":"tokens_per_min","remaining":100}}"#;
    match parse(line) {
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
    match parse(line) {
      Some(AgentEvent::Error { detail }) => {
        assert_eq!(detail, "boom");
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
      }]
    );
  }

  #[test]
  fn current_item_completed_agent_message_maps_to_message() {
    let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hello"}}"#;
    assert_eq!(parse_events(line), vec![AgentEvent::Message { text: "hello".into() }]);
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
        },
        AgentEvent::Completed,
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
      events.iter().filter(|e| matches!(e, AgentEvent::Completed)).count() == 1,
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
      match parse(line) {
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
