//! `issues:` and `issue:` sections of the Workflow Definition.
//!
//! Two separate top-level keys map to two separate concerns. `issues`
//! (plural) holds the intake pull command. `issue` (singular) holds
//! per-issue handling: hooks and the named stages the orchestrator
//! dispatches against `issue.state`. Splitting them keeps intake config
//! editable without touching the stage map.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
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

  /// `IndexMap` preserves author order so `should_dispatch` can iterate
  /// stages in workflow-file order — multiple stages may match the same
  /// state, and authors expect deterministic launch order.
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
  pub when: IssueStageMatch,
  pub agent: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub prompt_file: Option<PathBuf>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub prompt: Option<String>,
  #[serde(default)]
  pub hooks: IssueStageHooks,

  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

#[derive(Debug, Clone, Copy)]
pub enum IssueStagePromptSource<'a> {
  File(&'a Path),
  Inline(&'a str),
}

#[cfg(test)]
impl IssueStageSchema {
  pub fn new(when: impl Into<String>) -> Self {
    Self {
      when: IssueStageMatch {
        state: when.into(),
        unknown_fields: Default::default(),
      },
      agent: String::new(),
      prompt_file: None,
      prompt: None,
      hooks: IssueStageHooks::default(),
      unknown_fields: Default::default(),
    }
  }

  pub fn with_prompt_file(mut self, prompt_file: impl Into<PathBuf>) -> Self {
    self.prompt_file = Some(prompt_file.into());
    self
  }

  pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
    self.prompt = Some(prompt.into());
    self
  }
}

impl IssueStageSchema {
  pub fn prompt_source(&self) -> Option<IssueStagePromptSource<'_>> {
    match (&self.prompt_file, &self.prompt) {
      (Some(path), None) if !path.as_os_str().is_empty() => Some(IssueStagePromptSource::File(path)),
      (None, Some(prompt)) if !prompt.trim().is_empty() => Some(IssueStagePromptSource::Inline(prompt)),
      _ => None,
    }
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
      self.stages.iter().for_each(|(stage_name, stage)| {
        diagnostics.extends_with_pointer(&format!("stages.{stage_name}"), stage.diagnose(schema));
      });
    }

    diagnose_fields!(diagnostics, self, schema, "hooks" => hooks);
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
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

    if let Some(prompt_file) = &self.prompt_file {
      diagnostics.error_if_empty_path("prompt_file", prompt_file);
    }
    if let Some(prompt) = &self.prompt {
      diagnostics.error_if_empty_str("prompt", prompt);
    }
    match (&self.prompt_file, &self.prompt) {
      (None, None) => diagnostics.push(Diagnostic::error(
        "prompt",
        DiagnosticCode::MissingOneOf(vec!["prompt_file".into(), "prompt".into()]),
      )),
      (Some(_), Some(_)) => diagnostics.push(Diagnostic::error(
        "prompt",
        DiagnosticCode::MutuallyExclusiveFields(vec!["prompt_file".into(), "prompt".into()]),
      )),
      _ => {},
    }
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

  fn workflow_with_agent() -> WorkflowSchema {
    let mut workflow = WorkflowSchema::default();
    workflow.agents.insert(
      "codex".to_string(),
      AgentProfileSchema::new(AgentRuntime::Codex, "gpt-5.5".to_string()),
    );
    workflow
  }

  #[test]
  fn issue_stage_accepts_prompt_file_source() {
    let workflow = workflow_with_agent();
    let stage: IssueStageSchema = serde_yaml::from_str(
      r#"
when:
  state: Todo
agent: codex
prompt_file: ./prompts/plan.md
"#,
    )
    .expect("stage schema parses");

    let diagnostics = stage.diagnose(&workflow);

    assert!(!diagnostics.has_errors());
    assert!(
      matches!(stage.prompt_source(), Some(IssueStagePromptSource::File(path)) if path == Path::new("./prompts/plan.md"))
    );
  }

  #[test]
  fn issue_stage_accepts_inline_prompt_source() {
    let workflow = workflow_with_agent();
    let stage: IssueStageSchema = serde_yaml::from_str(
      r#"
when:
  state: Todo
agent: codex
prompt: |
  Plan issue {{ issue.id }}
"#,
    )
    .expect("stage schema parses");

    let diagnostics = stage.diagnose(&workflow);

    assert!(!diagnostics.has_errors());
    assert!(
      matches!(stage.prompt_source(), Some(IssueStagePromptSource::Inline(prompt)) if prompt.contains("Plan issue"))
    );
  }

  #[test]
  fn issue_stage_rejects_both_prompt_sources() {
    let workflow = workflow_with_agent();
    let stage: IssueStageSchema = serde_yaml::from_str(
      r#"
when:
  state: Todo
agent: codex
prompt_file: ./prompts/plan.md
prompt: |
  Plan issue {{ issue.id }}
"#,
    )
    .expect("stage schema parses");

    let diagnostics = stage.diagnose(&workflow);

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "prompt" && matches!(diag.code, DiagnosticCode::MutuallyExclusiveFields(_)) })
    );
    assert!(stage.prompt_source().is_none());
  }

  #[test]
  fn issue_stage_rejects_missing_prompt_source() {
    let workflow = workflow_with_agent();
    let stage: IssueStageSchema = serde_yaml::from_str(
      r#"
when:
  state: Todo
agent: codex
"#,
    )
    .expect("stage schema parses");

    let diagnostics = stage.diagnose(&workflow);

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "prompt" && matches!(diag.code, DiagnosticCode::MissingOneOf(_)) })
    );
    assert!(stage.prompt_source().is_none());
  }

  #[test]
  fn issue_stage_rejects_empty_prompt_file() {
    let workflow = workflow_with_agent();
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
    assert!(stage.prompt_source().is_none());
  }

  #[test]
  fn issue_stage_rejects_empty_inline_prompt() {
    let workflow = workflow_with_agent();
    let stage: IssueStageSchema = serde_yaml::from_str(
      r#"
when:
  state: Todo
agent: codex
prompt: '   '
"#,
    )
    .expect("stage schema parses");

    let diagnostics = stage.diagnose(&workflow);

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "prompt" && matches!(diag.code, DiagnosticCode::EmptyStr) })
    );
    assert!(stage.prompt_source().is_none());
  }

  #[test]
  fn issue_stage_accepts_known_agent_without_unknown_agent_error() {
    let workflow = workflow_with_agent();
    let stage: IssueStageSchema = serde_yaml::from_str(
      r#"
when:
  state: Todo
agent: codex
prompt_file: ./prompts/plan.md
"#,
    )
    .expect("stage schema parses");

    let diagnostics = stage.diagnose(&workflow);

    assert!(
      !diagnostics
        .errors
        .iter()
        .any(|diag| matches!(diag.code, DiagnosticCode::UnknownAgent(_)))
    );
  }
}
