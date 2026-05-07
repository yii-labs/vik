//! `issues:` and `issue:` sections of the Workflow Definition.
//!
//! Two separate top-level keys map to two separate concerns. `issues`
//! (plural) holds the intake pull command. `issue` (singular) holds
//! per-issue handling: hooks and the named stages the orchestrator
//! dispatches against `issue.state`. Splitting them keeps intake config
//! editable without touching the stage map.

use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::WorkflowSchema;
use super::diagnose::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueIntakeSchema {
  pub pull: IssuePullSchema,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuePullSchema {
  pub command: String,

  #[serde(default = "default_idle_sec")]
  pub idle_sec: u64,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

fn default_idle_sec() -> u64 {
  5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueHandlingSchema {
  /// Cross-cutting hooks; run for every matched issue regardless of which
  /// stages fire. `after_create` is the only kind today.
  #[serde(default)]
  pub hooks: IssueHooks,

  /// `IndexMap` preserves author order so `should_dispatch` can iterate
  /// stages in workflow-file order — multiple stages may match the same
  /// state, and authors expect deterministic launch order.
  pub stages: IndexMap<String, IssueStage>,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueHooks {
  /// Runs once after the issue workdir is created and before any stage
  /// session spawns. Must be idempotent: every matched intake cycle reruns
  /// it because Vik keeps no per-issue marker file.
  pub after_create: Option<String>,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueStage {
  pub when: IssueStageMatch,
  pub agent: String,
  pub prompt_file: PathBuf,
  #[serde(default)]
  pub hooks: IssueStageHooks,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueStageMatch {
  /// Compared with case-sensitive equality to `issue.state`. Vik never
  /// normalizes tracker states; what the tracker reports is what matches.
  pub state: String,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueStageHooks {
  /// Failure aborts the stage before the session spawns.
  pub before_run: Option<String>,
  /// Skipped on cancellation; failure is logged but does not propagate.
  pub after_run: Option<String>,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

impl Diagnose for IssueIntakeSchema {
  fn diagnose(&self, schema: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnose_fields!(diagnostics, self, schema, "pull" => pull);
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

impl Diagnose for IssuePullSchema {
  fn diagnose(&self, _: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnostics.error_if_empty_str("command", &self.command);
    diagnostics.error_if_non_positive("idle_sec", self.idle_sec as usize);
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

impl Diagnose for IssueHandlingSchema {
  fn diagnose(&self, schema: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnostics.error_if_empty_map("stages", self.stages.is_empty());
    if !self.stages.is_empty() {
      self.stages.iter().for_each(|(stage_name, stage)| {
        diagnostics.extends_with_pointer(&format!("stages.{stage_name}"), stage.diagnose(schema));
      });
    }

    diagnose_fields!(diagnostics, self, schema, "hooks" => hooks);
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

impl Diagnose for IssueStage {
  fn diagnose(&self, schema: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnose_fields!(
      diagnostics,
      self,
      schema,
      "when" => when,
      "hooks" => hooks,
    );
    diagnostics.error_if_empty_str("agent", &self.agent);

    // Stage's `agent` must reference an entry in the top-level `agents`
    // map; without this check, a typo would be caught only at session
    // spawn time, after intake and hooks have already run.
    if !self.agent.trim().is_empty() && !schema.agents.contains_key(&self.agent) {
      diagnostics.push(Diagnostic::error(
        "agent",
        DiagnosticCode::UnknownAgent(self.agent.clone()),
      ));
    }

    diagnostics.error_if_empty_path("prompt_file", &self.prompt_file);
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

impl Diagnose for IssueHooks {
  fn diagnose(&self, _: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

impl Diagnose for IssueStageMatch {
  fn diagnose(&self, _: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnostics.error_if_empty_str("state", &self.state);
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

impl Diagnose for IssueStageHooks {
  fn diagnose(&self, _: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}
