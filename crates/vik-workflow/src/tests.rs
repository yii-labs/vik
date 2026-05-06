use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde_yaml::Mapping;
use tempfile::tempdir;
use vik_core::{HostPlatform, Issue, WorkflowDefinition};

use super::*;

fn sample_issue() -> Issue {
    Issue {
        id: "id1".into(),
        identifier: "ABC-1".into(),
        title: "Do work".into(),
        description: None,
        priority: Some(1),
        state: "Todo".into(),
        branch_name: None,
        url: None,
        labels: vec!["bug".into()],
        blocked_by: vec![],
        created_at: None,
        updated_at: None,
    }
}

#[test]
fn parses_front_matter_and_prompt() {
    let parsed = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\n---\nHello {{ issue.identifier }}\n",
    )
    .unwrap();
    assert_eq!(parsed.prompt_template, "Hello {{ issue.identifier }}");
    assert!(
        parsed
            .config
            .contains_key(serde_yaml::Value::String("tracker".to_string()))
    );
}

#[test]
fn rejects_non_map_front_matter() {
    let err =
        parse_workflow_content(PathBuf::from("WORKFLOW.md"), "---\n- bad\n---\nBody").unwrap_err();
    assert!(matches!(err, WorkflowError::WorkflowFrontMatterNotAMap));
}

#[test]
fn applies_defaults_and_path_resolution() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("WORKFLOW.md");
    fs::write(
        &path,
        "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\nworkspace:\n  root: work\nagent:\n  max_concurrent_agents_by_state:\n    TODO: 2\n    Bad: 0\ncodex:\n  approvals_reviewer: auto_review\n---\nBody",
    )
    .unwrap();
    let def = parse_workflow_file(&path).unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    assert_eq!(config.polling.interval_ms, 30_000);
    assert_eq!(config.agent.runtime, AgentRuntimeConfig::Codex);
    assert_eq!(config.codex.read_timeout_ms, 30_000);
    assert_eq!(config.workspace.root, dir.path().join("work"));
    assert_eq!(config.logging.dir, dir.path().join("work").join("logs"));
    assert_eq!(
        config.agent.max_concurrent_agents_by_state.get("todo"),
        Some(&2)
    );
    assert_eq!(
        config.codex.approvals_reviewer,
        Some(serde_json::Value::String("auto_review".to_string()))
    );
    assert!(config.tracker.filter().assignees.is_empty());
    assert!(config.tracker.filter().tags.is_empty());
    assert!(config.tracker.github_provider().is_none());
    assert!(
        !config
            .agent
            .max_concurrent_agents_by_state
            .contains_key("bad")
    );
}

#[test]
fn parses_agent_runtime() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\nagent:\n  runtime: codex\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();

    assert_eq!(config.agent.runtime, AgentRuntimeConfig::Codex);
}

#[test]
fn rejects_unsupported_agent_runtime() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\nagent:\n  runtime: other\n---\nBody",
    )
    .unwrap();
    let err = ServiceConfig::from_definition(&def).unwrap_err();

    assert!(matches!(
        err,
        WorkflowError::InvalidConfig(message) if message == "unsupported agent.runtime: other"
    ));
}

#[test]
fn parses_tracker_filter() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("WORKFLOW.md");
    fs::write(
        &path,
        "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\n  filter:\n    assignees:\n      - user-a\n      - user-b\n    tags:\n      - agent\n      - codex\nworkspace:\n  root: work\n---\nBody",
    )
    .unwrap();

    let def = parse_workflow_file(&path).unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();

    assert_eq!(config.tracker.filter().assignees, vec!["user-a", "user-b"]);
    assert_eq!(config.tracker.filter().tags, vec!["agent", "codex"]);
}

#[test]
fn empty_tracker_filter_lists_match_all_issues() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("WORKFLOW.md");
    fs::write(
        &path,
        "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\n  filter:\n    assignees: []\n    tags: []\nworkspace:\n  root: work\n---\nBody",
    )
    .unwrap();

    let def = parse_workflow_file(&path).unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();

    assert!(config.tracker.filter().assignees.is_empty());
    assert!(config.tracker.filter().tags.is_empty());
}

#[test]
fn accepts_github_tracker_config() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("WORKFLOW.md");
    fs::write(
        &path,
        "---\ntracker:\n  kind: github\n  api_key: gh_token\n  repository: yii-labs/vik\n  active_states: [Todo, In Progress]\n  terminal_states: [Done, Closed]\nworkspace:\n  root: work\n---\nBody",
    )
    .unwrap();

    let def = parse_workflow_file(&path).unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();

    assert_eq!(config.tracker.kind_name(), "github");
    let provider = config.tracker.github_provider().unwrap();
    assert_eq!(provider.endpoint, "https://api.github.com");
    assert_eq!(provider.api_key, "gh_token");
    assert_eq!(provider.repository, "yii-labs/vik");
    assert!(config.tracker.linear_provider().is_none());
    config.validate_for_dispatch().unwrap();
}

#[test]
fn github_tracker_requires_repository() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: github\n  api_key: gh_token\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let err = config.validate_for_dispatch().unwrap_err();

    assert!(matches!(err, WorkflowError::MissingTrackerRepository));
}

#[test]
fn github_tracker_rejects_malformed_repository() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: github\n  api_key: gh_token\n  repository: yii-labs\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let err = config.validate_for_dispatch().unwrap_err();

    assert!(matches!(err, WorkflowError::InvalidTrackerRepository(_)));
}

#[test]
fn unsupported_tracker_kind_is_rejected_for_dispatch() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: jira\n  api_key: token\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let err = config.validate_for_dispatch().unwrap_err();

    assert!(matches!(err, WorkflowError::UnsupportedTrackerKind));
}

#[test]
fn resolves_explicit_logging_dir() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("WORKFLOW.md");
    fs::write(
        &path,
        "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\nworkspace:\n  root: work\nlogging:\n  dir: logs\n---\nBody",
    )
    .unwrap();

    let def = parse_workflow_file(&path).unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();

    assert_eq!(config.logging.dir, dir.path().join("logs"));
}

#[test]
fn parses_codex_model_fields() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\ncodex:\n  command: codex --config shell_environment_policy.inherit=all app-server\n  model: gpt-5.5\n  model_reasoning_effort: xhigh\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    assert_eq!(
        config.codex.command,
        "codex --config shell_environment_policy.inherit=all app-server"
    );
    assert_eq!(config.codex.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(
        config.codex.model_reasoning_effort.as_deref(),
        Some("xhigh")
    );
}

#[test]
fn codex_command_program_preserves_windows_paths() {
    let config = CodexConfig {
        command: r#"C:\Users\me\bin\codex.exe app-server"#.to_string(),
        ..CodexConfig::default()
    };

    assert_eq!(
        config
            .command_program_for_platform(HostPlatform::Windows)
            .as_deref(),
        Some(r#"C:\Users\me\bin\codex.exe"#)
    );
}

#[test]
fn codex_command_program_preserves_quoted_windows_paths() {
    let config = CodexConfig {
        command: r#""C:\Program Files\Codex\codex.exe" app-server"#.to_string(),
        ..CodexConfig::default()
    };

    assert_eq!(
        config
            .command_program_for_platform(HostPlatform::Windows)
            .as_deref(),
        Some(r#"C:\Program Files\Codex\codex.exe"#)
    );
}

#[test]
fn codex_command_program_ignores_empty_quoted_program() {
    let config = CodexConfig {
        command: "'' app-server".to_string(),
        ..CodexConfig::default()
    };

    assert_eq!(
        config.command_program_for_platform(HostPlatform::Posix),
        None
    );
}

#[test]
fn codex_command_program_skips_posix_environment_assignments() {
    let config = CodexConfig {
        command: r#"CODEX_HOME=/tmp/codex OPENAI_API_KEY="token value" codex app-server"#
            .to_string(),
        ..CodexConfig::default()
    };

    assert_eq!(
        config
            .command_program_for_platform(HostPlatform::Posix)
            .as_deref(),
        Some("codex")
    );
}

#[test]
fn rejects_model_fields_without_app_server_command() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\ncodex:\n  command: codex exec\n  model: gpt-5.5\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let err = config.validate_for_dispatch().unwrap_err();
    assert!(matches!(
        err,
        WorkflowError::InvalidConfig(message)
            if message == "codex.command must include app-server when codex.model or codex.model_reasoning_effort is set"
    ));
}

#[test]
fn diagnosis_reports_missing_linear_key_as_warning() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\n  api_key: \"\"\n  project_slug: proj\nworkspace:\n  root: work\nhooks:\n  after_create: |\n    git clone git@github.com:yii-labs/vik .\ncodex:\n  command: codex app-server\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let environment = MockDiagnoseEnvironment::new()
        .with_env("GH_TOKEN")
        .with_commands(["codex", "gh", "git"])
        .with_success("codex", ["login", "status"]);

    let diagnoses = config.diagnose(&environment);

    assert!(!diagnoses.has_errors());
    assert_diagnosis(&diagnoses, "config.tracker", DiagnosisSeverity::Passed);
    assert_diagnosis(
        &diagnoses,
        "env.tracker_api_key",
        DiagnosisSeverity::Warning,
    );
    assert_diagnosis(&diagnoses, "command.codex", DiagnosisSeverity::Passed);
}

#[test]
fn diagnosis_reports_config_shape_errors_as_errors() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: github\n  api_key: \"\"\n  repository: yii-labs\ncodex:\n  command: codex app-server\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let environment = MockDiagnoseEnvironment::new().with_commands(["codex", "gh", "git"]);

    let diagnoses = config.diagnose(&environment);

    assert!(diagnoses.has_errors());
    assert_diagnosis(&diagnoses, "config.tracker", DiagnosisSeverity::Error);
    assert_diagnosis(
        &diagnoses,
        "env.tracker_api_key",
        DiagnosisSeverity::Warning,
    );
}

#[test]
fn diagnosis_checks_command_authentication() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\ncodex:\n  command: codex app-server\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let environment = MockDiagnoseEnvironment::new().with_commands(["codex", "gh", "git"]);

    let diagnoses = config.diagnose(&environment);

    assert!(!diagnoses.has_errors());
    assert_diagnosis(&diagnoses, "auth.codex", DiagnosisSeverity::Warning);
    assert_diagnosis(&diagnoses, "auth.github", DiagnosisSeverity::Warning);
}

#[test]
fn diagnosis_checks_codex_cmd_authentication() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\ncodex:\n  command: codex.cmd app-server\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let environment = MockDiagnoseEnvironment::new()
        .with_env("GH_TOKEN")
        .with_commands(["codex.cmd", "gh", "git"])
        .with_success("codex.cmd", ["login", "status"]);

    let diagnoses = config.diagnose(&environment);

    assert_diagnosis(&diagnoses, "auth.codex", DiagnosisSeverity::Passed);
}

#[test]
fn diagnosis_skips_hook_environment_assignments() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\nhooks:\n  after_create: |\n    GIT_SSH_COMMAND=ssh git clone git@github.com:yii-labs/vik .\ncodex:\n  command: codex app-server\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let environment = MockDiagnoseEnvironment::new()
        .with_env("GH_TOKEN")
        .with_commands(["codex", "gh", "git"])
        .with_success("codex", ["login", "status"]);

    let diagnoses = config.diagnose(&environment);

    assert_diagnosis(&diagnoses, "command.git", DiagnosisSeverity::Passed);
    assert!(
        diagnoses
            .iter()
            .all(|diagnosis| diagnosis.name != "command.GIT_SSH_COMMAND=ssh")
    );
}

#[test]
fn diagnosis_skips_codex_command_environment_assignments() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\ncodex:\n  command: CODEX_HOME=/tmp/codex codex app-server\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let environment = MockDiagnoseEnvironment::new()
        .with_env("GH_TOKEN")
        .with_commands(["codex", "gh", "git"])
        .with_success("codex", ["login", "status"]);

    let diagnoses = config.diagnose(&environment);

    assert_diagnosis(&diagnoses, "command.codex", DiagnosisSeverity::Passed);
    assert_diagnosis(&diagnoses, "auth.codex", DiagnosisSeverity::Passed);
    assert!(
        diagnoses
            .iter()
            .all(|diagnosis| diagnosis.name != "command.CODEX_HOME=/tmp/codex")
    );
}

#[test]
fn dispatch_validation_still_requires_tracker_api_key() {
    let def = parse_workflow_content(
        PathBuf::from("WORKFLOW.md"),
        "---\ntracker:\n  kind: linear\n  api_key: \"\"\n  project_slug: proj\n---\nBody",
    )
    .unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();
    let err = config.validate_for_dispatch().unwrap_err();

    assert!(matches!(err, WorkflowError::MissingTrackerApiKey));
}

#[test]
fn strict_prompt_render_fails_on_unknown() {
    let def = WorkflowDefinition {
        path: PathBuf::from("WORKFLOW.md"),
        config: Mapping::new(),
        prompt_template: "Hello {{ missing }}".to_string(),
    };
    let err = render_prompt(&def, &sample_issue(), None).unwrap_err();
    assert!(matches!(err, WorkflowError::TemplateRenderError(_)));
}

#[test]
fn prompt_renders_issue_and_attempt() {
    let def = WorkflowDefinition {
        path: PathBuf::from("WORKFLOW.md"),
        config: Mapping::new(),
        prompt_template: "{{ issue.identifier }} attempt={{ attempt }}".to_string(),
    };
    let rendered = render_prompt(&def, &sample_issue(), Some(2)).unwrap();
    assert_eq!(rendered, "ABC-1 attempt=2");
}

#[derive(Debug, Default)]
struct MockDiagnoseEnvironment {
    env: BTreeSet<String>,
    commands: BTreeSet<String>,
    successes: BTreeSet<String>,
}

impl MockDiagnoseEnvironment {
    fn new() -> Self {
        Self::default()
    }

    fn with_env(mut self, name: impl Into<String>) -> Self {
        self.env.insert(name.into());
        self
    }

    fn with_commands(mut self, commands: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.commands.extend(commands.into_iter().map(Into::into));
        self
    }

    fn with_success(
        mut self,
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.successes.insert(command_key(program, args));
        self
    }
}

impl DiagnoseEnvironment for MockDiagnoseEnvironment {
    fn env_var_is_set(&self, name: &str) -> bool {
        self.env.contains(name)
    }

    fn command_exists(&self, command: &str) -> bool {
        self.commands.contains(command)
    }

    fn command_succeeds(&self, program: &str, args: &[&str]) -> bool {
        self.successes
            .contains(&command_key(program, args.iter().copied()))
    }
}

fn command_key(
    program: impl Into<String>,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> String {
    let mut key = program.into();
    for arg in args {
        key.push('\0');
        key.push_str(&arg.into());
    }
    key
}

fn assert_diagnosis(diagnoses: &Diagnoses, name: &str, severity: DiagnosisSeverity) {
    let diagnosis = diagnoses
        .iter()
        .find(|diagnosis| diagnosis.name == name)
        .unwrap_or_else(|| panic!("missing diagnosis {name} in {diagnoses:?}"));
    assert_eq!(diagnosis.severity, severity);
}
