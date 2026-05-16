use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Session event vocabulary.
///
/// Session JSONL stores provider-neutral events. Adapter-owned provider
/// structs may attach the full provider JSON to these events through
/// `raw`, so session files keep durable evidence without exposing
/// Codex- or Claude-specific variants here. Parse errors on a single
/// JSONL line surface as [`AgentEvent::Error`] because
/// forward-compatible evidence beats a hard fail on one future provider
/// event shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
  /// Emitted as early as the provider allows so the session layer can
  /// stamp the JSONL filename or tracing span without buffering events.
  SessionStarted {
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<Value>,
  },

  Message {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<Value>,
  },

  ToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<Value>,
  },

  /// Adapters may emit this multiple times per run; the session
  /// accumulates with `saturating_add` to absorb retries.
  TokenUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<Value>,
  },

  /// `scope` is provider-qualified (e.g. `codex:tokens_per_min`) so
  /// observations from different runtimes never collide in the session
  /// snapshot.
  RateLimit {
    scope: String,
    remaining: u64,
    reset_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<Value>,
  },

  Completed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<Value>,
  },

  /// Valid provider JSONL that has no recognized snapshot meaning yet.
  Unknown {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    event_type: Option<String>,
    raw: Value,
  },

  /// Either a JSONL parse failure on one line or a provider-side error.
  /// The stream keeps going; the session decides whether to mark the
  /// run as Failed.
  Error {
    detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<Value>,
  },
}

impl AgentEvent {
  pub fn affects_snapshot(&self) -> bool {
    !matches!(self, AgentEvent::ToolCall { .. } | AgentEvent::Unknown { .. })
  }

  #[cfg(test)]
  pub fn raw(&self) -> Option<&Value> {
    match self {
      AgentEvent::SessionStarted { raw, .. }
      | AgentEvent::Message { raw, .. }
      | AgentEvent::ToolCall { raw, .. }
      | AgentEvent::TokenUsage { raw, .. }
      | AgentEvent::RateLimit { raw, .. }
      | AgentEvent::Completed { raw }
      | AgentEvent::Error { raw, .. } => raw.as_ref(),
      AgentEvent::Unknown { raw, .. } => Some(raw),
    }
  }
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
          raw: None,
        },
        json!({
          "kind": "session_started",
          "session_id": "session-1"
        }),
      ),
      (
        AgentEvent::Message {
          text: "stage output".into(),
          raw: None,
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
          raw: None,
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
          raw: None,
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
        AgentEvent::Completed { raw: None },
        json!({
          "kind": "completed"
        }),
      ),
      (
        AgentEvent::Error {
          detail: "provider line failed to decode".into(),
          raw: None,
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
          raw: None,
        },
      ),
      (
        json!({
          "kind": "message",
          "text": "stage output"
        }),
        AgentEvent::Message {
          text: "stage output".into(),
          raw: None,
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
          raw: None,
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
          raw: None,
        },
      ),
      (
        json!({
          "kind": "completed"
        }),
        AgentEvent::Completed { raw: None },
      ),
      (
        json!({
          "kind": "error",
          "detail": "provider line failed to decode"
        }),
        AgentEvent::Error {
          detail: "provider line failed to decode".into(),
          raw: None,
        },
      ),
    ];

    for (value, expected) in cases {
      let event: AgentEvent = serde_json::from_value(value.clone()).expect("event deserializes");

      assert_eq!(event, expected);
      assert_eq!(serde_json::to_value(event).expect("event serializes"), value);
    }
  }

  #[test]
  fn raw_provider_data_roundtrips_on_neutral_events() {
    let cases = [
      (
        AgentEvent::ToolCall {
          id: Some("tool_0".into()),
          name: Some("shell".into()),
          input: Some(json!("{}")),
          raw: Some(json!({
            "type": "item.completed",
            "item": {
              "id": "tool_0",
              "type": "tool_call",
              "name": "shell",
              "arguments": "{}"
            }
          })),
        },
        json!({
          "kind": "tool_call",
          "id": "tool_0",
          "name": "shell",
          "input": "{}",
          "raw": {
            "type": "item.completed",
            "item": {
              "id": "tool_0",
              "type": "tool_call",
              "name": "shell",
              "arguments": "{}"
            }
          }
        }),
      ),
      (
        AgentEvent::Unknown {
          event_type: Some("future.event".into()),
          raw: json!({
            "type": "future.event",
            "payload": {
              "ok": true
            }
          }),
        },
        json!({
          "kind": "unknown",
          "event_type": "future.event",
          "raw": {
            "type": "future.event",
            "payload": {
              "ok": true
            }
          }
        }),
      ),
      (
        AgentEvent::Message {
          text: "hello".into(),
          raw: Some(json!({
            "type": "item.completed",
            "item": {
              "type": "agent_message",
              "text": "hello"
            }
          })),
        },
        json!({
          "kind": "message",
          "text": "hello",
          "raw": {
            "type": "item.completed",
            "item": {
              "type": "agent_message",
              "text": "hello"
            }
          }
        }),
      ),
      (
        AgentEvent::Completed {
          raw: Some(json!({
            "type": "turn.completed"
          })),
        },
        json!({
          "kind": "completed",
          "raw": {
            "type": "turn.completed"
          }
        }),
      ),
      (
        AgentEvent::Error {
          detail: "boom".into(),
          raw: Some(json!({
            "type": "error",
            "message": "boom"
          })),
        },
        json!({
          "kind": "error",
          "detail": "boom",
          "raw": {
            "type": "error",
            "message": "boom"
          }
        }),
      ),
    ];

    for (event, expected) in cases {
      let value = serde_json::to_value(&event).expect("event serializes");

      assert_eq!(value, expected);
      assert_eq!(
        serde_json::from_value::<AgentEvent>(value).expect("event deserializes"),
        event
      );
    }
  }
}
