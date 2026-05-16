//! `agents:` section of the Workflow Definition.
//!
//! Each map entry parses into an [`AgentProfileSchema`] holding a runtime
//! tag, a model name, and an opaque `args` mapping. The agent module owns
//! the typed param structs because adapters define what they accept; the
//! config layer is intentionally the lower layer that imports them, so
//! adding a new runtime never forces a config-side change.
//!
//! ```yaml
//! agents:
//!   codex:
//!     runtime: codex
//!     model: gpt-5.5
//!     args:
//!       --config:
//!         - model_reasoning_effort=high
//! ```

use std::fmt::Display;
use std::ops::Deref;
use std::ops::DerefMut;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::WorkflowSchema;
use super::diagnose::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntime {
  Codex,
  ClaudeCode,
}

impl AsRef<str> for AgentRuntime {
  fn as_ref(&self) -> &str {
    match self {
      AgentRuntime::Codex => "codex",
      AgentRuntime::ClaudeCode => "claude_code",
    }
  }
}

impl Display for AgentRuntime {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", self.as_ref())
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfileSchema {
  pub runtime: AgentRuntime,
  /// Interpretation is runtime-specific: a Codex model name like `gpt-5.5`,
  /// a Claude alias like `opus`, etc. Vik does not validate the value.
  pub model: String,

  /// Forwarded verbatim to the spawn command. Kept as `serde_yaml::Mapping`
  /// (not a typed struct) so each runtime can accept its own flag set
  /// without expanding this schema.
  #[serde(default, skip_serializing_if = "serde_yaml::Mapping::is_empty")]
  pub args: serde_yaml::Mapping,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

#[cfg(test)]
impl AgentProfileSchema {
  pub fn new(runtime: AgentRuntime, model: String) -> Self {
    Self {
      runtime,
      model,
      args: serde_yaml::Mapping::new(),
      unknown_fields: serde_yaml::Mapping::new(),
    }
  }

  pub fn with_args(mut self, args: serde_yaml::Mapping) -> Self {
    self.args = args;
    self
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentProfilesSchema(IndexMap<String, AgentProfileSchema>);

impl Diagnose for AgentProfileSchema {
  fn diagnose(&self, _: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnostics.error_if_empty_str("model", &self.model);
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

impl Diagnose for AgentProfilesSchema {
  fn diagnose(&self, workflow: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnostics.error_if_empty_map_here(self.is_empty());
    self.0.iter().for_each(|(profile_name, profile)| {
      diagnostics.extends_with_pointer(profile_name, profile.diagnose(workflow));
    });

    diagnostics
  }
}

impl Deref for AgentProfilesSchema {
  type Target = IndexMap<String, AgentProfileSchema>;

  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl DerefMut for AgentProfilesSchema {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.0
  }
}
