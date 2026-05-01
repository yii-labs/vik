use serde_json::json;
use vik_workflow::CodexConfig;

use crate::client::build_codex_command;
use crate::event::extract_usage;

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
fn thread_start_payload_uses_workspace_cwd() {
    let payload = json!({
        "cwd": "/tmp/workspace",
        "ephemeral": true
    });
    assert_eq!(payload["cwd"], "/tmp/workspace");
}

#[test]
fn codex_command_inserts_dynamic_model_config_before_app_server() {
    let config = CodexConfig {
        command: "codex --config shell_environment_policy.inherit=all app-server".into(),
        model: Some("gpt-5.5".into()),
        model_reasoning_effort: Some("xhigh".into()),
        ..CodexConfig::default()
    };
    assert_eq!(
        build_codex_command(&config),
        "codex --config shell_environment_policy.inherit=all --config 'model=\"gpt-5.5\"' --config 'model_reasoning_effort=\"xhigh\"' app-server"
    );
}

#[test]
fn codex_command_keeps_base_command_without_dynamic_config() {
    let config = CodexConfig {
        command: "codex app-server".into(),
        ..CodexConfig::default()
    };
    assert_eq!(build_codex_command(&config), "codex app-server");
}
