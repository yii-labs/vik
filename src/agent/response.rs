use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
  /// Exact parsed Codex JSONL event plus the recognized Codex event
  /// type. Unknown future Codex event types are still retained.
  CodexProviderEvent {
    event_type: CodexProviderEventKind,
    event: Value,
  },

  /// Exact parsed Claude Code JSONL event plus the recognized Claude
  /// Code event type. Unknown future Claude event types are still
  /// retained.
  ClaudeCodeProviderEvent {
    event_type: ClaudeCodeProviderEventKind,
    event: Value,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodexProviderEventKind {
  SessionConfigured,
  AgentMessage,
  TokenCount,
  RateLimitWarning,
  RateLimitReset,
  TurnComplete,
  ShutdownComplete,
  ThreadStarted,
  ItemCompleted { item_type: Option<String> },
  TurnCompleted,
  Error,
  Unknown { event_type: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClaudeCodeProviderEventKind {
  System { subtype: Option<String> },
  Assistant { content_types: Vec<String> },
  User,
  Result,
  Unknown { event_type: Option<String> },
}
