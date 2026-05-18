//! Typed mirror of `workflow.yml`.
//!
//! Each sub-module owns one top-level section (`loop`, `workspace`, `agents`,
//! `issues`, `issue`). The schema is parse-only here; resolved paths and the
//! hook runner live in [`crate::workflow::Workflow`]. That split lets
//! `vik doctor` validate config without pulling in the agent registry.
pub mod agent;
pub mod diagnose;
pub mod issue;
pub mod loop_;
pub mod workspace;

use serde::{Deserialize, Serialize};

pub use agent::*;
use diagnose::*;
pub use issue::*;
pub use loop_::*;
pub use workspace::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSchema {
  #[serde(rename = "loop")]
  pub loop_: LoopSchema,
  pub workspace: WorkspaceSchema,
  pub agents: AgentProfilesSchema,
  pub issues: IssueIntakeSchema,
  pub issue: IssueHandlingSchema,

  /// Every sub-schema preserves unmodeled keys via `#[serde(flatten)]` so the
  /// doctor can warn on typos and operators on newer YAML keep round-tripping
  /// instead of failing parse.
  #[serde(flatten)]
  unknown_fields: serde_yaml::Mapping,
}

impl WorkflowSchema {
  pub fn diagnose(&self) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();

    diagnose_fields!(
      diagnostics,
      self,
      self,
      "loop" => loop_,
      "workspace" => workspace,
      "agents" => agents,
      "issues" => issues,
      "issue" => issue,
    );
    diagnostics.warn_unknown_fields(&self.unknown_fields);

    diagnostics
  }
}

impl Default for WorkflowSchema {
  fn default() -> Self {
    Self {
      loop_: LoopSchema::default(),
      workspace: WorkspaceSchema::default(),
      agents: AgentProfilesSchema::default(),
      issues: IssueIntakeSchema::default(),
      issue: IssueHandlingSchema::default(),
      unknown_fields: serde_yaml::Mapping::new(),
    }
  }
}

#[cfg(test)]
mod tests {
  use std::path::Path;
  use std::path::PathBuf;

  use super::diagnose::DiagnosticCode;
  use super::*;

  const VALID_WORKFLOW: &str = r#"
loop:
  max_issue_concurrency: 2
  wait_ms: 100
workspace:
  root: workspace
agents:
  codex:
    runtime: codex
    model: gpt-5.5
    args:
      --config:
        - model_reasoning_effort=high
issues:
  pull:
    command: ./scripts/issues-json
    idle_sec: 5
issue:
  hooks:
    after_create: echo created
  stages:
    plan:
      when:
        state: todo
      agent: codex
      prompt_file: ./prompts/plan.md
      hooks:
        before_run: echo before
        after_run: echo after
    implement:
      when:
        state: todo
      agent: codex
      prompt_file: ./prompts/implement.md
"#;

  #[test]
  fn workflow_schema_parses_core_sections_and_preserves_stage_order() {
    let schema = parse_schema(VALID_WORKFLOW);

    assert_eq!(schema.loop_.max_issue_concurrency, 2);
    assert_eq!(schema.loop_.wait_ms, 100);
    assert_eq!(schema.workspace.root.as_deref(), Some(Path::new("workspace")));
    assert_eq!(schema.issues.pull.command, "./scripts/issues-json");
    assert_eq!(schema.issues.pull.idle_sec, 5);
    assert_eq!(
      schema.issue.stages.iter().map(|stage| stage.name.as_str()).collect::<Vec<_>>(),
      ["plan", "implement"]
    );
    assert_eq!(schema.issue.hooks.after_create.as_deref(), Some("echo created"));
    let plan = schema
      .issue
      .stages
      .iter()
      .find(|stage| stage.name == "plan")
      .expect("plan stage");
    assert_eq!(plan.hooks.before_run.as_deref(), Some("echo before"));
    assert_eq!(plan.hooks.after_run.as_deref(), Some("echo after"));
  }

  #[test]
  fn diagnostics_include_nested_pointers_for_invalid_schema() {
    let mut schema = WorkflowSchema::default();
    schema.loop_.max_issue_concurrency = 0;
    schema.loop_.wait_ms = 0;
    schema.loop_.max_iterations = Some(0);
    schema.workspace.root = Some(PathBuf::new());
    schema.agents.insert(
      "codex".to_string(),
      AgentProfileSchema::new(AgentRuntime::Codex, String::new()),
    );
    schema.issues.pull.command = String::new();
    schema.issues.pull.idle_sec = 0;

    let mut stage = IssueStageSchema::new("").with_name("plan");
    stage.agent = "missing".to_string();
    schema.issue.stages.push(stage);

    let diagnostics = schema.diagnose();

    assert!(diagnostics.has_errors());
    assert!(diagnostics.errors.iter().any(|diag| {
      diag.pointer == "loop.max_issue_concurrency" && matches!(diag.code, DiagnosticCode::NonPositiveNumber(0))
    }));
    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "workspace.root" && matches!(diag.code, DiagnosticCode::EmptyStr) })
    );
    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "agents.codex.model" && matches!(diag.code, DiagnosticCode::EmptyStr) })
    );
    assert!(diagnostics.errors.iter().any(|diag| {
      diag.pointer == "issue.stages.plan.agent"
        && matches!(&diag.code, DiagnosticCode::UnknownAgent(agent) if agent == "missing")
    }));
    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "issues.pull.command" && matches!(diag.code, DiagnosticCode::EmptyStr) })
    );
    assert!(diagnostics.errors.iter().any(|diag| {
      diag.pointer == "issues.pull.idle_sec" && matches!(diag.code, DiagnosticCode::NonPositiveNumber(0))
    }));
  }

  #[test]
  fn agents_schema_reports_empty_map_at_agents_pointer() {
    let schema = WorkflowSchema::default();

    let diagnostics = schema.diagnose();

    assert!(
      diagnostics
        .errors
        .iter()
        .any(|diag| { diag.pointer == "agents" && matches!(diag.code, DiagnosticCode::EmptyMap) })
    );
    assert!(
      !diagnostics.errors.iter().any(|diag| diag.pointer == "agents."),
      "empty child pointers must resolve to the parent pointer"
    );
  }

  #[test]
  fn unknown_fields_surface_as_warnings() {
    let schema = parse_schema(
      r#"
loop:
  max_issue_concurrency: 1
  wait_ms: 100
  extra_loop_field: true
workspace:
  root: workspace
  extra_workspace_field: true
extra_top_field: true
agents:
  codex:
    runtime: codex
    model: gpt-5.5
    args: {}
    extra_agent_field: true
issues:
  pull:
    command: ./scripts/issues-json
    idle_sec: 5
    extra_pull_field: true
  extra_issues_field: true
issue:
  hooks:
    extra_issue_hook_field: true
  extra_issue_field: true
  stages:
    plan:
      when:
        state: todo
        extra_when_field: true
      agent: codex
      prompt_file: ./prompts/plan.md
      hooks:
        extra_stage_hook_field: true
      extra_stage_field: true
"#,
    );

    let diagnostics = schema.diagnose();

    assert!(!diagnostics.has_errors());
    assert!(
      diagnostics
        .warnings
        .iter()
        .any(|diag| { diag.pointer == "extra_top_field" && matches!(diag.code, DiagnosticCode::UnknownField) })
    );
    assert!(
      diagnostics
        .warnings
        .iter()
        .any(|diag| { diag.pointer == "loop.extra_loop_field" && matches!(diag.code, DiagnosticCode::UnknownField) })
    );
    assert!(diagnostics.warnings.iter().any(|diag| {
      diag.pointer == "workspace.extra_workspace_field" && matches!(diag.code, DiagnosticCode::UnknownField)
    }));
    assert!(diagnostics.warnings.iter().any(|diag| {
      diag.pointer == "agents.codex.extra_agent_field" && matches!(diag.code, DiagnosticCode::UnknownField)
    }));
    assert!(diagnostics.warnings.iter().any(|diag| {
      diag.pointer == "issues.extra_issues_field" && matches!(diag.code, DiagnosticCode::UnknownField)
    }));
    assert!(diagnostics.warnings.iter().any(|diag| {
      diag.pointer == "issues.pull.extra_pull_field" && matches!(diag.code, DiagnosticCode::UnknownField)
    }));
    assert!(
      diagnostics
        .warnings
        .iter()
        .any(|diag| { diag.pointer == "issue.extra_issue_field" && matches!(diag.code, DiagnosticCode::UnknownField) })
    );
    assert!(diagnostics.warnings.iter().any(|diag| {
      diag.pointer == "issue.hooks.extra_issue_hook_field" && matches!(diag.code, DiagnosticCode::UnknownField)
    }));
    assert!(diagnostics.warnings.iter().any(|diag| {
      diag.pointer == "issue.stages.plan.extra_stage_field" && matches!(diag.code, DiagnosticCode::UnknownField)
    }));
    assert!(diagnostics.warnings.iter().any(|diag| {
      diag.pointer == "issue.stages.plan.when.extra_when_field" && matches!(diag.code, DiagnosticCode::UnknownField)
    }));
    assert!(diagnostics.warnings.iter().any(|diag| {
      diag.pointer == "issue.stages.plan.hooks.extra_stage_hook_field"
        && matches!(diag.code, DiagnosticCode::UnknownField)
    }));
  }

  #[test]
  fn documented_flat_runtime_shape_parses() {
    let profile: AgentProfileSchema = serde_yaml::from_str(
      r#"
model: opus
runtime: claude_code
args:
  --any-arg: high
"#,
    )
    .expect("documented flat runtime profile parses");

    assert!(matches!(profile.runtime, AgentRuntime::ClaudeCode));
  }

  fn parse_schema(contents: &str) -> WorkflowSchema {
    serde_yaml::from_str(contents).expect("workflow schema parses")
  }
}
