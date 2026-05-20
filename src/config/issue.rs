//! `issues:` and `issue:` sections of the Workflow Definition.
//!
//! Two separate top-level keys map to two separate concerns. `issues`
//! (plural) holds intake sources. `issue` (singular) holds
//! per-issue handling: hooks and the named stages the orchestrator
//! dispatches against `issue.state`. Splitting them keeps intake config
//! editable without touching the stage map.

use std::path::PathBuf;

use indexmap::IndexMap;
use serde::de;
use serde::{Deserialize, Deserializer, Serialize};

use super::WorkflowSchema;
use super::diagnose::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueIntakeSchema {
  #[serde(default)]
  pub pull: Option<IssuePullSchema>,
  #[serde(default)]
  pub webhook: Option<IssueWebhookSchema>,

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
pub struct IssueWebhookSchema {
  #[serde(default, rename = "x-event-signature")]
  pub x_event_signature: Option<String>,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueHandlingSchema {
  /// Cross-cutting hooks; run for every matched issue regardless of which
  /// stages fire. `after_create` is the only kind today.
  #[serde(default)]
  pub hooks: IssueHooks,

  /// Authored YAML stays a name-keyed map. Runtime storage keeps that ordered
  /// map and duplicates each map key into the stage value.
  #[serde(deserialize_with = "deserialize_stages")]
  pub stages: IndexMap<String, IssueStageSchema>,

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
  #[serde(flatten, deserialize_with = "deserialize_prompt_source")]
  pub prompt_source: IssueStagePromptSource,
  #[serde(default)]
  pub hooks: IssueStageHooks,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStagePromptSource {
  #[serde(rename = "prompt_file")]
  File(PathBuf),
  #[serde(rename = "prompt")]
  Inline(String),
}

#[derive(Deserialize)]
struct IssueStagePromptSourceInput {
  #[serde(default)]
  prompt_file: Option<PathBuf>,
  #[serde(default)]
  prompt: Option<String>,
}

fn deserialize_prompt_source<'de, D>(deserializer: D) -> Result<IssueStagePromptSource, D::Error>
where
  D: Deserializer<'de>,
{
  let input = IssueStagePromptSourceInput::deserialize(deserializer)?;
  match (input.prompt_file, input.prompt) {
    (Some(prompt_file), None) => Ok(IssueStagePromptSource::File(prompt_file)),
    (None, Some(prompt)) => Ok(IssueStagePromptSource::Inline(prompt)),
    (Some(_), Some(_)) | (None, None) => Err(de::Error::custom(
      "issue stage must define exactly one of `prompt_file` or `prompt`",
    )),
  }
}

impl Default for IssueStagePromptSource {
  fn default() -> Self {
    Self::File(PathBuf::new())
  }
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
      prompt_source: IssueStagePromptSource::default(),
      hooks: IssueStageHooks::default(),
      unknown_fields: Default::default(),
    }
  }

  pub fn with_name(mut self, name: impl Into<String>) -> Self {
    self.name = name.into();
    self
  }

  pub fn with_prompt_file(mut self, prompt_file: impl Into<PathBuf>) -> Self {
    self.prompt_source = IssueStagePromptSource::File(prompt_file.into());
    self
  }

  pub fn with_inline_prompt(mut self, prompt: impl Into<String>) -> Self {
    self.prompt_source = IssueStagePromptSource::Inline(prompt.into());
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

    if self.pull.is_none() && self.webhook.is_none() {
      diagnostics.error_if_empty_map_here(true);
    }
    if let Some(pull) = &self.pull {
      diagnostics.extends_with_pointer("pull", pull.diagnose(schema));
    }
    if let Some(webhook) = &self.webhook {
      diagnostics.extends_with_pointer("webhook", webhook.diagnose(schema));
    }
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

impl Diagnose for IssueWebhookSchema {
  fn diagnose(&self, _: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    if let Some(signature) = &self.x_event_signature {
      diagnostics.error_if_empty_str("x-event-signature", signature);
    }
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

impl Diagnose for IssueHandlingSchema {
  fn diagnose(&self, schema: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnostics.error_if_empty_map("stages", self.stages.is_empty());
    if !self.stages.is_empty() {
      self.stages.iter().for_each(|(name, stage)| {
        diagnostics.error_if_empty_str("stages", name);
        diagnostics.extends_with_pointer(&stage_pointer(name), stage.diagnose(schema));
      });
    }

    diagnose_fields!(diagnostics, self, schema, "hooks" => hooks);
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

fn deserialize_stages<'de, D>(deserializer: D) -> Result<IndexMap<String, IssueStageSchema>, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let mut stages = IndexMap::<String, IssueStageSchema>::deserialize(deserializer)?;
  stages.iter_mut().for_each(|(name, stage)| {
    stage.name = name.clone();
  });

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

    diagnostics.extends_with_pointer("", self.prompt_source.diagnose(schema));
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

impl Diagnose for IssueStagePromptSource {
  fn diagnose(&self, _: &WorkflowSchema) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    match self {
      IssueStagePromptSource::File(prompt_file) => diagnostics.error_if_empty_path("prompt_file", prompt_file),
      IssueStagePromptSource::Inline(prompt) => diagnostics.error_if_empty_str("prompt", prompt),
    }

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
  fn issue_intake_accepts_pull_only() {
    let intake: IssueIntakeSchema = serde_yaml::from_str(
      r#"
pull:
  command: ./scripts/issues-json
"#,
    )
    .expect("intake schema parses");

    let diagnostics = intake.diagnose(&WorkflowSchema::default());

    assert!(intake.pull.is_some());
    assert!(intake.webhook.is_none());
    assert!(!diagnostics.has_errors(), "{diagnostics}");
  }

  #[test]
  fn issue_intake_accepts_webhook_only_with_signature() {
    let intake: IssueIntakeSchema = serde_yaml::from_str(
      r#"
webhook:
  x-event-signature: shared-secret
"#,
    )
    .expect("intake schema parses");

    let diagnostics = intake.diagnose(&WorkflowSchema::default());

    assert!(intake.pull.is_none());
    assert_eq!(
      intake.webhook.as_ref().and_then(|webhook| webhook.x_event_signature.as_deref()),
      Some("shared-secret")
    );
    assert!(!diagnostics.has_errors(), "{diagnostics}");
  }

  #[test]
  fn issue_intake_accepts_pull_and_webhook_together() {
    let intake: IssueIntakeSchema = serde_yaml::from_str(
      r#"
pull:
  command: ./scripts/issues-json
webhook: {}
"#,
    )
    .expect("intake schema parses");

    let diagnostics = intake.diagnose(&WorkflowSchema::default());

    assert!(intake.pull.is_some());
    assert!(intake.webhook.is_some());
    assert!(!diagnostics.has_errors(), "{diagnostics}");
  }

  #[test]
  fn issue_intake_requires_pull_or_webhook() {
    let intake: IssueIntakeSchema = serde_yaml::from_str("{}").expect("intake schema parses");

    let diagnostics = intake.diagnose(&WorkflowSchema::default());

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer.is_empty() && matches!(diag.code, DiagnosticCode::EmptyMap) })
    );
  }

  #[test]
  fn issue_webhook_reports_empty_signature() {
    let webhook: IssueWebhookSchema = serde_yaml::from_str(
      r#"
x-event-signature: ''
"#,
    )
    .expect("webhook schema parses");

    let diagnostics = webhook.diagnose(&WorkflowSchema::default());

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "x-event-signature" && matches!(diag.code, DiagnosticCode::EmptyStr) })
    );
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
  fn issue_stages_deserialize_map_into_named_indexmap_entries() {
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
      issue.stages.keys().map(String::as_str).collect::<Vec<_>>(),
      ["plan", "implement"]
    );
    assert_eq!(issue.stages.get("plan").expect("plan stage").name, "plan");
    assert_eq!(
      issue.stages.get("implement").expect("implement stage").name,
      "implement"
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

    assert_eq!(issue.stages.get("plan").expect("plan stage").name, "plan");
    assert!(
      diagnostics
        .warnings
        .iter()
        .any(|diag| { diag.pointer == "stages.plan.name" && matches!(diag.code, DiagnosticCode::UnknownField) })
    );
  }

  #[test]
  fn issue_stage_prompt_source_accepts_prompt_file() {
    let stage: IssueStageSchema = serde_yaml::from_str(
      r#"
when:
  state: Todo
agent: codex
prompt_file: ./prompts/plan.md
"#,
    )
    .expect("stage schema parses");

    let IssueStagePromptSource::File(path) = stage.prompt_source else {
      panic!("expected file prompt source");
    };

    assert_eq!(path, PathBuf::from("./prompts/plan.md"));
  }

  #[test]
  fn issue_stage_prompt_source_accepts_inline_prompt() {
    let stage: IssueStageSchema = serde_yaml::from_str(
      r#"
when:
  state: Todo
agent: codex
prompt: |
  plan on {{ issue.id }}
"#,
    )
    .expect("stage schema parses");

    let diagnostics = stage.diagnose(&workflow_with_agent());

    assert!(!diagnostics.has_errors(), "{diagnostics}");
    assert!(!diagnostics.has_warnings(), "{diagnostics}");
  }

  #[test]
  fn issue_stage_prompt_source_rejects_both_sources() {
    let err = serde_yaml::from_str::<IssueStageSchema>(
      r#"
when:
  state: Todo
agent: codex
prompt_file: ./prompts/plan.md
prompt: inline
"#,
    )
    .expect_err("both prompt sources must fail");

    assert!(err.to_string().contains("prompt_file"));
    assert!(err.to_string().contains("prompt"));
  }

  #[test]
  fn issue_stage_prompt_source_rejects_missing_source() {
    let err = serde_yaml::from_str::<IssueStageSchema>(
      r#"
when:
  state: Todo
agent: codex
"#,
    )
    .expect_err("missing prompt source must fail");

    assert!(err.to_string().contains("prompt_file"));
    assert!(err.to_string().contains("prompt"));
  }

  #[test]
  fn issue_stage_prompt_source_preserves_unknown_field_warning() {
    let stage: IssueStageSchema = serde_yaml::from_str(
      r#"
when:
  state: Todo
agent: codex
prompt: inline
extra_stage_field: true
"#,
    )
    .expect("stage schema parses");

    let diagnostics = stage.diagnose(&workflow_with_agent());

    assert!(!diagnostics.has_errors(), "{diagnostics}");
    assert!(
      diagnostics
        .warnings
        .iter()
        .any(|diag| { diag.pointer == "extra_stage_field" && matches!(diag.code, DiagnosticCode::UnknownField) })
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
