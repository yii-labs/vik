use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::adapters::{ClaudeCodeEvent, CodexEvent};

/// Session event vocabulary.
///
/// Session JSONL stores both typed provider JSONL records and decoded
/// provider-agnostic semantic events. A run yields at most one
/// [`AgentEvent::SessionStarted`], any number of typed provider
/// records plus `Message`/`TokenUsage`/`RateLimit` semantic events
/// interleaved, and terminates with one [`AgentEvent::Completed`] (or
/// trailing `Error`). Parse errors on a single JSONL line surface as
/// [`AgentEvent::Error`] because forward-compatible evidence beats a
/// hard fail on one future provider event shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
  /// Exact parsed Codex JSONL event. Unknown future Codex event types
  /// are still retained by the Codex adapter.
  CodexProviderEvent {
    event: CodexEvent,
  },

  /// Exact parsed Claude Code JSONL event. Unknown future Claude event
  /// types are still retained by the Claude Code adapter.
  ClaudeCodeProviderEvent {
    event: ClaudeCodeEvent,
  },

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

  Completed,

  /// Either a JSONL parse failure on one line or a provider-side error.
  /// The stream keeps going; the session decides whether to mark the
  /// run as Failed.
  Error {
    detail: String,
  },
}

impl AgentEvent {
  pub fn is_provider_record(&self) -> bool {
    matches!(
      self,
      AgentEvent::CodexProviderEvent { .. } | AgentEvent::ClaudeCodeProviderEvent { .. }
    )
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use crate::agent::{ClaudeCodeContentBlock, ClaudeCodeEventKind, CodexEventKind};

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

  #[test]
  fn typed_provider_events_roundtrip() {
    let cases = [
      (
        AgentEvent::CodexProviderEvent {
          event: CodexEvent {
            kind: CodexEventKind::ItemCompleted {
              item_type: Some("tool_call".into()),
              item: Some(json!({
                "type": "tool_call"
              })),
            },
            raw: json!({
              "type": "item.completed",
              "item": {
                "type": "tool_call"
              }
            }),
          },
        },
        json!({
          "kind": "codex_provider_event",
          "event": {
            "kind": "item_completed",
            "item_type": "tool_call",
            "item": {
              "type": "tool_call"
            },
            "raw": {
              "type": "item.completed",
              "item": {
                "type": "tool_call"
              }
            }
          }
        }),
      ),
      (
        AgentEvent::ClaudeCodeProviderEvent {
          event: ClaudeCodeEvent {
            kind: ClaudeCodeEventKind::Assistant {
              content: vec![ClaudeCodeContentBlock {
                block_type: "tool_use".into(),
                text: None,
                raw: json!({
                  "type": "tool_use"
                }),
              }],
            },
            raw: json!({
              "type": "assistant",
              "message": {
                "content": [
                  {
                    "type": "tool_use"
                  }
                ]
              }
            }),
          },
        },
        json!({
          "kind": "claude_code_provider_event",
          "event": {
            "kind": "assistant",
            "content": [
              {
                "type": "tool_use",
                "text": null,
                "raw": {
                  "type": "tool_use"
                }
              }
            ],
            "raw": {
              "type": "assistant",
              "message": {
                "content": [
                  {
                    "type": "tool_use"
                  }
                ]
              }
            }
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
