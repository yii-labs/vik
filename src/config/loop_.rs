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

#[cfg(test)]
mod tests {
  use crate::config::WorkflowSchema;
  use crate::config::diagnose::Diagnose;
  use crate::config::diagnose::DiagnosticCode;

  use super::*;

  #[test]
  fn loop_schema_defaults_to_continuous_polling_limits() {
    let loop_schema = LoopSchema::default();

    let diagnostics = loop_schema.diagnose(&WorkflowSchema::default());

    assert_eq!(loop_schema.max_issue_concurrency, 10);
    assert_eq!(loop_schema.wait_ms, 5000);
    assert_eq!(loop_schema.max_iterations, None);
    assert!(!diagnostics.has_errors());
    assert!(!diagnostics.has_warnings());
  }

  #[test]
  fn loop_schema_diagnoses_zero_limits_and_unknown_fields() {
    let loop_schema: LoopSchema = serde_yaml::from_str(
      r#"
max_issue_concurrency: 0
wait_ms: 0
max_iterations: 0
typo: true
"#,
    )
    .expect("loop schema parses");

    let diagnostics = loop_schema.diagnose(&WorkflowSchema::default());

    assert!(diagnostics.errors.iter().any(|diag| {
      diag.pointer == "max_issue_concurrency" && matches!(diag.code, DiagnosticCode::NonPositiveNumber(0))
    }));
    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| diag.pointer == "wait_ms" && matches!(diag.code, DiagnosticCode::NonPositiveNumber(0)))
    );
    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "max_iterations" && matches!(diag.code, DiagnosticCode::NonPositiveNumber(0)) })
    );
    assert!(
      diagnostics
        .warnings
        .iter()
        .any(|diag| diag.pointer == "typo" && matches!(diag.code, DiagnosticCode::UnknownField))
    );
  }
}
