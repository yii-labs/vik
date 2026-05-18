//! Plain value types for an immutable session view.
//!
//! [`SessionSnapshot`] is owned by the session task. Callers can request
//! a one-time clone through the session command channel; normal progress
//! uses [`SessionState`] only.

use std::{collections::HashMap, fmt::Display};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
  #[default]
  UnStarted,
  Preparing,
  Running,
  Completed,
  Failed,
  Cancelled,
}

impl Display for SessionState {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let s = match self {
      SessionState::UnStarted => "unstarted",
      SessionState::Preparing => "preparing",
      SessionState::Running => "running",
      SessionState::Completed => "completed",
      SessionState::Failed => "failed",
      SessionState::Cancelled => "cancelled",
    };

    write!(f, "{s}")
  }
}

impl SessionState {
  pub fn is_terminated(self) -> bool {
    matches!(
      self,
      SessionState::Completed | SessionState::Failed | SessionState::Cancelled
    )
  }
}

/// Provider may emit `TokenUsage` more than once per run; the session
/// accumulates these via `saturating_add`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
  pub input: u64,
  pub output: u64,
  pub cache_read: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitObservation {
  pub remaining: u64,
  pub reset_at: DateTime<Utc>,
  /// Used for latest-wins comparison: a stale observation arriving
  /// after a fresh one (provider retry) is dropped.
  pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionSnapshot {
  pub agent_session_id: Option<String>,
  pub state: SessionState,
  pub started_at: DateTime<Utc>,
  pub last_event_at: Option<DateTime<Utc>>,
  pub last_message: Option<String>,
  pub tokens: TokenUsage,
  /// Keyed by provider scope (e.g. `codex:tokens_per_min`). Multiple
  /// scopes can be in flight simultaneously, so a flat map suits the
  /// streaming nature better than a fixed struct.
  pub rate_limits: HashMap<String, RateLimitObservation>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn session_state_reports_only_terminal_states_as_terminated() {
    for state in [SessionState::UnStarted, SessionState::Preparing, SessionState::Running] {
      assert!(!state.is_terminated());
    }

    for state in [SessionState::Completed, SessionState::Failed, SessionState::Cancelled] {
      assert!(state.is_terminated());
    }
  }
}
