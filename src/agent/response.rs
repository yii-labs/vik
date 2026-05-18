use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Provider-agnostic event vocabulary.
///
/// Adapters translate provider-specific JSONL into this enum. Semantic
/// events keep the session snapshot small; observation events
/// (`ToolCall`, `Subagent`, and `Unknown`) keep the full parsed
/// provider JSON in `raw` so session JSONL remains useful when a
/// provider ships a new event shape. A run yields at most one
/// [`AgentEvent::SessionStarted`], any number of
/// `Message`/`TokenUsage`/`RateLimit` interleaved, and terminates with
/// one [`AgentEvent::Completed`] (or trailing `Error`). Parse errors on
/// a single JSONL line surface as [`AgentEvent::Error`] rather than
/// tearing the subprocess down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
  /// Emitted as early as the provider allows so the session layer can
  /// stamp the JSONL filename or tracing span without buffering events.
  SessionStarted {
    session_id: String,
  },

  Message {
    text: String,
  },

  /// Adapters may emit this multiple times per run; the session
  /// accumulates with `saturating_add` to absorb retries.
  TokenUsage {
    input: u64,
    output: u64,
    cache_read: u64,
  },

  /// `scope` is provider-qualified (e.g. `codex:tokens_per_min`) so
  /// observations from different runtimes never collide in the session
  /// snapshot.
  RateLimit {
    scope: String,
    remaining: u64,
    reset_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
  },

  ToolCall {
    call_id: Option<String>,
    name: Option<String>,
    phase: ToolCallPhase,
    input: Option<Value>,
    output: Option<Value>,
    raw: Value,
  },

  /// Delegation/subagent evidence from providers that expose it. Raw
  /// JSON carries provider-specific detail such as prompts, models, and
  /// agent states.
  Subagent {
    call_id: Option<String>,
    action: String,
    status: Option<String>,
    target_ids: Vec<String>,
    raw: Value,
  },

  /// Valid provider JSON that Vik does not model yet.
  Unknown {
    event_type: Option<String>,
    raw: Value,
  },

  Completed,

  /// Either a JSONL parse failure on one line or a provider-side error.
  /// The stream keeps going; the session decides whether to mark the
  /// run as Failed.
  Error {
    detail: String,
  },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallPhase {
  Request,
  Result,
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  fn utc(value: &str) -> DateTime<Utc> {
    value.parse().expect("test timestamp parses")
  }

  #[test]
  fn serializes_provider_neutral_event_vocabulary() {
    let cases = [
      (
        AgentEvent::SessionStarted {
          session_id: "session-1".into(),
        },
        json!({
          "kind": "session_started",
          "session_id": "session-1"
        }),
      ),
      (
        AgentEvent::Message {
          text: "stage output".into(),
        },
        json!({
          "kind": "message",
          "text": "stage output"
        }),
      ),
      (
        AgentEvent::TokenUsage {
          input: 11,
          output: 7,
          cache_read: 3,
        },
        json!({
          "kind": "token_usage",
          "input": 11,
          "output": 7,
          "cache_read": 3
        }),
      ),
      (
        AgentEvent::RateLimit {
          scope: "provider:tokens_per_min".into(),
          remaining: 42,
          reset_at: utc("2026-05-16T10:15:30Z"),
          observed_at: utc("2026-05-16T10:00:00Z"),
        },
        json!({
          "kind": "rate_limit",
          "scope": "provider:tokens_per_min",
          "remaining": 42,
          "reset_at": "2026-05-16T10:15:30Z",
          "observed_at": "2026-05-16T10:00:00Z"
        }),
      ),
      (
        AgentEvent::ToolCall {
          call_id: Some("tool-1".into()),
          name: Some("Bash".into()),
          phase: ToolCallPhase::Request,
          input: Some(json!({"command": "cargo test"})),
          output: None,
          raw: json!({"type": "assistant"}),
        },
        json!({
          "kind": "tool_call",
          "call_id": "tool-1",
          "name": "Bash",
          "phase": "request",
          "input": {"command": "cargo test"},
          "output": null,
          "raw": {"type": "assistant"}
        }),
      ),
      (
        AgentEvent::Subagent {
          call_id: Some("collab-1".into()),
          action: "spawnAgent".into(),
          status: Some("completed".into()),
          target_ids: vec!["thread-2".into()],
          raw: json!({"type": "collabAgentToolCall"}),
        },
        json!({
          "kind": "subagent",
          "call_id": "collab-1",
          "action": "spawnAgent",
          "status": "completed",
          "target_ids": ["thread-2"],
          "raw": {"type": "collabAgentToolCall"}
        }),
      ),
      (
        AgentEvent::Unknown {
          event_type: Some("future_event_kind".into()),
          raw: json!({"type": "future_event_kind"}),
        },
        json!({
          "kind": "unknown",
          "event_type": "future_event_kind",
          "raw": {"type": "future_event_kind"}
        }),
      ),
      (
        AgentEvent::Completed,
        json!({
          "kind": "completed"
        }),
      ),
      (
        AgentEvent::Error {
          detail: "provider line failed to decode".into(),
        },
        json!({
          "kind": "error",
          "detail": "provider line failed to decode"
        }),
      ),
    ];

    for (event, expected) in cases {
      let value = serde_json::to_value(event).expect("event serializes");

      assert_eq!(value, expected);
      assert!(value.get("provider").is_none());
      assert!(value.get("runtime").is_none());
    }
  }

  #[test]
  fn deserializes_provider_neutral_events_and_roundtrips() {
    let cases = [
      (
        json!({
          "kind": "session_started",
          "session_id": "session-1"
        }),
        AgentEvent::SessionStarted {
          session_id: "session-1".into(),
        },
      ),
      (
        json!({
          "kind": "message",
          "text": "stage output"
        }),
        AgentEvent::Message {
          text: "stage output".into(),
        },
      ),
      (
        json!({
          "kind": "token_usage",
          "input": 11,
          "output": 7,
          "cache_read": 3
        }),
        AgentEvent::TokenUsage {
          input: 11,
          output: 7,
          cache_read: 3,
        },
      ),
      (
        json!({
          "kind": "rate_limit",
          "scope": "provider:tokens_per_min",
          "remaining": 42,
          "reset_at": "2026-05-16T10:15:30Z",
          "observed_at": "2026-05-16T10:00:00Z"
        }),
        AgentEvent::RateLimit {
          scope: "provider:tokens_per_min".into(),
          remaining: 42,
          reset_at: utc("2026-05-16T10:15:30Z"),
          observed_at: utc("2026-05-16T10:00:00Z"),
        },
      ),
      (
        json!({
          "kind": "tool_call",
          "call_id": "tool-1",
          "name": "Bash",
          "phase": "request",
          "input": {"command": "cargo test"},
          "output": null,
          "raw": {"type": "assistant"}
        }),
        AgentEvent::ToolCall {
          call_id: Some("tool-1".into()),
          name: Some("Bash".into()),
          phase: ToolCallPhase::Request,
          input: Some(json!({"command": "cargo test"})),
          output: None,
          raw: json!({"type": "assistant"}),
        },
      ),
      (
        json!({
          "kind": "subagent",
          "call_id": "collab-1",
          "action": "spawnAgent",
          "status": "completed",
          "target_ids": ["thread-2"],
          "raw": {"type": "collabAgentToolCall"}
        }),
        AgentEvent::Subagent {
          call_id: Some("collab-1".into()),
          action: "spawnAgent".into(),
          status: Some("completed".into()),
          target_ids: vec!["thread-2".into()],
          raw: json!({"type": "collabAgentToolCall"}),
        },
      ),
      (
        json!({
          "kind": "unknown",
          "event_type": "future_event_kind",
          "raw": {"type": "future_event_kind"}
        }),
        AgentEvent::Unknown {
          event_type: Some("future_event_kind".into()),
          raw: json!({"type": "future_event_kind"}),
        },
      ),
      (
        json!({
          "kind": "completed"
        }),
        AgentEvent::Completed,
      ),
      (
        json!({
          "kind": "error",
          "detail": "provider line failed to decode"
        }),
        AgentEvent::Error {
          detail: "provider line failed to decode".into(),
        },
      ),
    ];

    for (value, expected) in cases {
      let event: AgentEvent = serde_json::from_value(value.clone()).expect("event deserializes");

      assert_eq!(event, expected);
      assert_eq!(serde_json::to_value(event).expect("event serializes"), value);
    }
  }
}
