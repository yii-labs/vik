use serde_json::json;
use std::path::Path;
use vik_workflow::{CodexConfig, TrackerConfig};

use crate::client::codex_spawn_command;
use crate::event::extract_usage;
use crate::process::{permission_approval_result, thread_start_params, turn_start_params};
use crate::tools::DynamicTools;

#[test]
fn token_usage_prefers_absolute_totals() {
    let event = json!({
        "method": "thread/tokenUsage/updated",
        "params": {
            "total_token_usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "total_tokens": 15
            }
        }
    });
    let usage = extract_usage("thread/tokenUsage/updated", &event).unwrap();
    assert_eq!(usage.total_tokens, 15);
}

#[test]
fn session_id_composes_thread_and_turn() {
    assert_eq!(
        vik_core::session_id("thread-1", "turn-2"),
        "thread-1-turn-2"
    );
}

#[test]
fn codex_spawn_command_inserts_model_config_before_app_server() {
    let config = CodexConfig {
        command: "codex --config shell_environment_policy.inherit=all app-server".to_string(),
        model: Some("gpt-5.5".to_string()),
        model_reasoning_effort: Some("xhigh".to_string()),
        ..CodexConfig::default()
    };
    assert_eq!(
        codex_spawn_command(&config),
        "codex --config shell_environment_policy.inherit=all --config 'model=\"gpt-5.5\"' --config 'model_reasoning_effort=xhigh' app-server"
    );
}

#[test]
fn codex_spawn_command_keeps_command_when_model_config_absent() {
    let config = CodexConfig {
        command: "codex --config shell_environment_policy.inherit=all app-server".to_string(),
        ..CodexConfig::default()
    };
    assert_eq!(codex_spawn_command(&config), config.command);
}

#[test]
fn thread_start_payload_uses_workspace_cwd() {
    let config = CodexConfig {
        approvals_reviewer: Some(json!("auto_review")),
        ..CodexConfig::default()
    };
    let payload = thread_start_params(
        Path::new("/tmp/workspace"),
        "VIK-7: optimize workflow codex config",
        &config,
        &DynamicTools::default(),
    );
    assert_eq!(payload["cwd"], "/tmp/workspace");
    assert_eq!(payload["approvalsReviewer"], "auto_review");
}

#[test]
fn thread_start_payload_includes_configured_dynamic_tools() {
    let tools = DynamicTools::from_tracker_config(&TrackerConfig {
        kind: "linear".to_string(),
        endpoint: "https://api.linear.app/graphql".to_string(),
        api_key: "lin_api_key".to_string(),
        project_slug: "VIK".to_string(),
        active_states: vec!["Todo".to_string()],
        terminal_states: vec!["Done".to_string()],
    });
    let payload = thread_start_params(
        Path::new("/tmp/workspace"),
        "VIK-7: optimize workflow codex config",
        &CodexConfig::default(),
        &tools,
    );
    assert_eq!(
        payload.pointer("/dynamicTools/0/name"),
        Some(&json!("linear_graphql"))
    );
}

#[test]
fn permission_approval_grants_requested_permissions_for_session() {
    let result = permission_approval_result(&json!({
        "permissions": {
            "fileSystem": { "write": ["/tmp/workspace/.git"] },
            "network": { "domains": ["api.github.com"] }
        }
    }));
    assert_eq!(result["scope"], "session");
    assert_eq!(
        result.pointer("/permissions/fileSystem/write/0"),
        Some(&json!("/tmp/workspace/.git"))
    );
    assert_eq!(
        result.pointer("/permissions/network/domains/0"),
        Some(&json!("api.github.com"))
    );
}

#[test]
fn turn_start_workspace_write_policy_includes_workspace_and_network() {
    let config = CodexConfig {
        turn_sandbox_policy: Some(json!({ "type": "workspaceWrite" })),
        ..CodexConfig::default()
    };
    let payload = turn_start_params(
        "thread-1",
        Path::new("/tmp/workspace"),
        "continue".to_string(),
        &config,
    );
    assert_eq!(
        payload.pointer("/sandboxPolicy/writableRoots/0"),
        Some(&json!("/tmp/workspace"))
    );
    assert_eq!(
        payload.pointer("/sandboxPolicy/networkAccess"),
        Some(&json!(true))
    );
    assert_eq!(
        payload.pointer("/sandboxPolicy/excludeTmpdirEnvVar"),
        Some(&json!(false))
    );
    assert_eq!(
        payload.pointer("/sandboxPolicy/excludeSlashTmp"),
        Some(&json!(false))
    );
}

#[test]
fn turn_start_external_sandbox_policy_is_preserved() {
    let config = CodexConfig {
        approvals_reviewer: Some(json!("auto_review")),
        turn_sandbox_policy: Some(json!({
            "type": "externalSandbox",
            "networkAccess": "enabled"
        })),
        ..CodexConfig::default()
    };
    let payload = turn_start_params(
        "thread-1",
        Path::new("/tmp/workspace"),
        "continue".to_string(),
        &config,
    );
    assert_eq!(
        payload.pointer("/sandboxPolicy/type"),
        Some(&json!("externalSandbox"))
    );
    assert_eq!(
        payload.pointer("/approvalsReviewer"),
        Some(&json!("auto_review"))
    );
    assert_eq!(
        payload.pointer("/sandboxPolicy/networkAccess"),
        Some(&json!("enabled"))
    );
}
