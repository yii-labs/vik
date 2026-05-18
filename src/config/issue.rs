//! `issues:` and `issue:` sections of the Workflow Definition.
//!
//! Two separate top-level keys map to two separate concerns. `issues`
//! (plural) holds the intake pull command. `issue` (singular) holds
//! per-issue handling: hooks and the named stages the orchestrator
//! dispatches against `issue.state`. Splitting them keeps intake config
//! editable without touching the stage map.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::WorkflowSchema;
use super::diagnose::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

impl Default for IssuePullSchema {
  fn default() -> Self {
    Self {
      command: String::new(),
      idle_sec: default_idle_sec(),
      unknown_fields: serde_yaml::Mapping::new(),
    }
  }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueHandlingSchema {
  /// Cross-cutting hooks; run for every matched issue regardless of which
  /// stages fire. `after_create` is the only kind today.
  #[serde(default)]
  pub hooks: IssueHooks,

  /// Authored YAML stays a name-keyed map. Runtime storage is a flat ordered
  /// list whose stage names are copied from the map keys.
  #[serde(deserialize_with = "deserialize_stages")]
  pub stages: Vec<IssueStageSchema>,

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
pub struct IssueStageSchema {
  /// Derived from the `issue.stages.<name>` map key. `name` is not a
  /// supported field inside a stage body; if authored there, it remains an
  /// unknown-field diagnostic.
  #[serde(skip)]
  pub name: String,
  pub when: IssueStageMatch,
  pub agent: String,
  pub prompt_file: PathBuf,
  #[serde(default)]
  pub hooks: IssueStageHooks,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

#[cfg(test)]
impl IssueStageSchema {
  pub fn new(when: impl Into<String>) -> Self {
    Self {
      name: String::new(),
      when: IssueStageMatch {
        state: when.into(),
        unknown_fields: Default::default(),
      },
      agent: String::new(),
      prompt_file: PathBuf::new(),
      hooks: IssueStageHooks::default(),
      unknown_fields: Default::default(),
    }
  }

  pub fn with_name(mut self, name: impl Into<String>) -> Self {
    self.name = name.into();
    self
  }

  pub fn with_prompt_file(mut self, prompt_file: impl Into<PathBuf>) -> Self {
    self.prompt_file = prompt_file.into();
    self
  }
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
      self.stages.iter().for_each(|stage| {
        diagnostics.error_if_empty_str("stages", &stage.name);
        diagnostics.extends_with_pointer(&stage_pointer(&stage.name), stage.diagnose(schema));
      });
    }

    diagnose_fields!(diagnostics, self, schema, "hooks" => hooks);
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

fn deserialize_stages<'de, D>(deserializer: D) -> Result<Vec<IssueStageSchema>, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let stages = indexmap::IndexMap::<String, IssueStageSchema>::deserialize(deserializer)?
    .into_iter()
    .map(|(name, mut stage)| {
      stage.name = name;
      stage
    })
    .collect();

  Ok(stages)
}

fn stage_pointer(stage_name: &str) -> String {
  if stage_name.trim().is_empty() {
    "stages".to_string()
  } else {
    format!("stages.{stage_name}")
  }
}

impl Diagnose for IssueStageSchema {
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

#[cfg(test)]
mod tests {
  use crate::config::AgentProfileSchema;
  use crate::config::AgentRuntime;
  use crate::config::WorkflowSchema;
  use crate::config::diagnose::Diagnose;
  use crate::config::diagnose::DiagnosticCode;

  use super::*;

  #[test]
  fn issue_pull_defaults_idle_sec_when_omitted() {
    let pull: IssuePullSchema = serde_yaml::from_str(
      r#"
command: ./scripts/issues-json
"#,
    )
    .expect("pull schema parses");

    let diagnostics = pull.diagnose(&WorkflowSchema::default());

    assert_eq!(pull.command, "./scripts/issues-json");
    assert_eq!(pull.idle_sec, 5);
    assert!(!diagnostics.has_errors());
  }

  #[test]
  fn issue_stage_accepts_known_agent_and_reports_empty_prompt_file() {
    let mut workflow = WorkflowSchema::default();
    workflow.agents.insert(
      "codex".to_string(),
      AgentProfileSchema::new(AgentRuntime::Codex, "gpt-5.5".to_string()),
    );
    let stage: IssueStageSchema = serde_yaml::from_str(
      r#"
when:
  state: Todo
agent: codex
prompt_file: ''
"#,
    )
    .expect("stage schema parses");

    let diagnostics = stage.diagnose(&workflow);

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "prompt_file" && matches!(diag.code, DiagnosticCode::EmptyStr) })
    );
    assert!(
      !diagnostics
        .errors
        .iter()
        .any(|diag| matches!(diag.code, DiagnosticCode::UnknownAgent(_)))
    );
  }

  #[test]
  fn issue_stages_deserialize_map_into_ordered_stage_entries() {
    let issue: IssueHandlingSchema = serde_yaml::from_str(
      r#"
stages:
  plan:
    when:
      state: todo
    agent: codex
    prompt_file: ./plan.md
  implement:
    when:
      state: todo
    agent: codex
    prompt_file: ./implement.md
"#,
    )
    .expect("issue schema parses");

    assert_eq!(
      issue.stages.iter().map(|stage| stage.name.as_str()).collect::<Vec<_>>(),
      ["plan", "implement"]
    );
  }

  #[test]
  fn issue_stages_reject_array_shape() {
    let err = serde_yaml::from_str::<IssueHandlingSchema>(
      r#"
stages:
  - name: plan
    when:
      state: todo
    agent: codex
    prompt_file: ./plan.md
"#,
    )
    .expect_err("array-shaped stages are unsupported");

    assert!(err.to_string().contains("invalid type: sequence"));
  }

  #[test]
  fn issue_stages_report_empty_map() {
    let issue: IssueHandlingSchema = serde_yaml::from_str(
      r#"
stages: {}
"#,
    )
    .expect("issue schema parses");

    let diagnostics = issue.diagnose(&workflow_with_agent());

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "stages" && matches!(diag.code, DiagnosticCode::EmptyMap) })
    );
  }

  #[test]
  fn issue_stages_report_empty_stage_name() {
    let issue: IssueHandlingSchema = serde_yaml::from_str(
      r#"
stages:
  "":
    when:
      state: todo
    agent: codex
    prompt_file: ./plan.md
"#,
    )
    .expect("issue schema parses");

    let diagnostics = issue.diagnose(&workflow_with_agent());

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "stages" && matches!(diag.code, DiagnosticCode::EmptyStr) })
    );
  }

  #[test]
  fn issue_stages_derive_name_from_map_key_not_authored_field() {
    let issue: IssueHandlingSchema = serde_yaml::from_str(
      r#"
stages:
  plan:
    name: authored
    when:
      state: todo
    agent: codex
    prompt_file: ./plan.md
"#,
    )
    .expect("issue schema parses");

    let diagnostics = issue.diagnose(&workflow_with_agent());

    assert_eq!(
      issue.stages.iter().find(|stage| stage.name == "plan").expect("plan stage").name,
      "plan"
    );
    assert!(
      diagnostics
        .warnings
        .iter()
        .any(|diag| { diag.pointer == "stages.plan.name" && matches!(diag.code, DiagnosticCode::UnknownField) })
    );
  }

  fn workflow_with_agent() -> WorkflowSchema {
    let mut workflow = WorkflowSchema::default();
    workflow.agents.insert(
      "codex".to_string(),
      AgentProfileSchema::new(AgentRuntime::Codex, "gpt-5.5".to_string()),
    );
    workflow
  }
}
