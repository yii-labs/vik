use std::fs;
use std::path::PathBuf;

use serde_yaml::Mapping;
use tempfile::tempdir;
use vik_core::{Issue, WorkflowDefinition};

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
    assert_eq!(config.codex.read_timeout_ms, 30_000);
    assert_eq!(config.workspace.root, dir.path().join("work"));
    assert_eq!(
        config.logging.dir,
        dir.path().join("work").join(".vik").join("logs")
    );
    assert_eq!(
        config.logging.service_dir,
        dir.path().join(".vik").join("service")
    );
    assert_eq!(
        config.agent.max_concurrent_agents_by_state.get("todo"),
        Some(&2)
    );
    assert_eq!(
        config.codex.approvals_reviewer,
        Some(serde_json::Value::String("auto_review".to_string()))
    );
    assert!(config.tracker.filter.assignees.is_empty());
    assert!(config.tracker.filter.tags.is_empty());
    assert!(
        !config
            .agent
            .max_concurrent_agents_by_state
            .contains_key("bad")
    );
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

    assert_eq!(config.tracker.filter.assignees, vec!["user-a", "user-b"]);
    assert_eq!(config.tracker.filter.tags, vec!["agent", "codex"]);
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

    assert!(config.tracker.filter.assignees.is_empty());
    assert!(config.tracker.filter.tags.is_empty());
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
fn resolves_explicit_service_logging_dir() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("WORKFLOW.md");
    fs::write(
        &path,
        "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\nworkspace:\n  root: work\nlogging:\n  service_dir: service-logs\n---\nBody",
    )
    .unwrap();

    let def = parse_workflow_file(&path).unwrap();
    let config = ServiceConfig::from_definition(&def).unwrap();

    assert_eq!(config.logging.service_dir, dir.path().join("service-logs"));
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
