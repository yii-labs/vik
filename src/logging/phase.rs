//! Workflow phase enum carried on every span.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Closed enum: adding a variant is effectively a config surface
/// change because operator dashboards filter on these strings. Reserve
/// new variants for genuinely new top-level control flows, not
/// sub-phases of an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
  Startup,
  Intake,
  Dispatch,
  StageRun,
  Hook,
  Server,
  Daemon,
}

impl Phase {
  /// Matches the serde `snake_case` rename so JSON and [`fmt::Display`]
  /// agree byte-for-byte.
  pub fn as_str(&self) -> &'static str {
    match self {
      Phase::Startup => "startup",
      Phase::Intake => "intake",
      Phase::Dispatch => "dispatch",
      Phase::StageRun => "stage_run",
      Phase::Hook => "hook",
      Phase::Server => "server",
      Phase::Daemon => "daemon",
    }
  }
}

impl fmt::Display for Phase {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(self.as_str())
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::Phase;

  fn phase_wire_cases() -> [(Phase, &'static str); 7] {
    [
      (Phase::Startup, "startup"),
      (Phase::Intake, "intake"),
      (Phase::Dispatch, "dispatch"),
      (Phase::StageRun, "stage_run"),
      (Phase::Hook, "hook"),
      (Phase::Server, "server"),
      (Phase::Daemon, "daemon"),
    ]
  }

  #[test]
  fn phase_display_matches_stable_wire_strings() {
    for (phase, wire) in phase_wire_cases() {
      assert_eq!(phase.to_string(), wire);
      assert_eq!(phase.as_str(), wire);
    }
  }

  #[test]
  fn phase_serde_matches_stable_wire_strings() {
    for (phase, wire) in phase_wire_cases() {
      assert_eq!(serde_json::to_value(phase).expect("phase serializes"), json!(wire));
      assert_eq!(
        serde_json::from_value::<Phase>(json!(wire)).expect("phase deserializes"),
        phase
      );
    }
  }
}
