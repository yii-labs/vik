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
