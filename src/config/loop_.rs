//! `loop:` section of the Workflow Definition.

use serde::{Deserialize, Serialize};

use super::WorkflowSchema;
use super::diagnose::*;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoopSchema {
  /// Cap on distinct issue ids running concurrently. A single
  /// issue with several matching stages still counts as one — this is the
  /// safety valve against runaway intake, not against per-stage fan-out.
  #[serde(default = "default_max_issue_concurrency")]
  pub max_issue_concurrency: u32,

  #[serde(default = "default_wait_ms")]
  pub wait_ms: u64,

  /// `None` runs forever. Operators opt into a finite cap explicitly so
  /// nobody gets a "the orchestrator silently exited" surprise.
  #[serde(default)]
  pub max_iterations: Option<u64>,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

fn default_max_issue_concurrency() -> u32 {
  10
}

fn default_wait_ms() -> u64 {
  5000
}

impl Default for LoopSchema {
  fn default() -> Self {
    Self {
      max_issue_concurrency: default_max_issue_concurrency(),
      wait_ms: default_wait_ms(),
      max_iterations: None,
      unknown_fields: serde_yaml::Mapping::new(),
    }
  }
}

impl Diagnose for LoopSchema {
  fn diagnose(&self, _: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnostics.error_if_non_positive("max_issue_concurrency", self.max_issue_concurrency as usize);
    diagnostics.error_if_non_positive("wait_ms", self.wait_ms as usize);

    if let Some(max_iterations) = self.max_iterations {
      diagnostics.error_if_non_positive("max_iterations", max_iterations as usize);
    }

    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}
