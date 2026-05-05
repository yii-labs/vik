use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use vik_core::{
    HostPlatform, Issue, IssueAttachment, IssueComment, IssueTracker, IssueUpdate, TrackerError,
};
use vik_workflow::CodexConfig;

use crate::client::{
    codex_spawn_command, codex_spawn_process_command_for_platform, message_belongs_to_turn,
};
use crate::event::extract_usage;
use crate::process::{permission_approval_result, thread_start_params, turn_start_params};
use crate::session_log::{SessionLog, session_log_dir, session_log_path};
use crate::tools::DynamicTools;

#[derive(Debug)]
struct TestTracker;

#[async_trait::async_trait]
impl IssueTracker for TestTracker {
    async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError> {
        Ok(vec![])
    }

    async fn fetch_by_states(&self, _state_names: &[String]) -> Result<Vec<Issue>, TrackerError> {
        Ok(vec![])
    }

    async fn fetch_states_by_ids(&self, _issue_ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        Ok(vec![])
    }

    async fn get_issue(&self, issue_id: &str) -> Result<Issue, TrackerError> {
        Ok(test_issue(issue_id))
    }

    async fn update_issue(
        &self,
        issue_id: &str,
        _update: IssueUpdate,
    ) -> Result<Issue, TrackerError> {
        Ok(test_issue(issue_id))
    }

    async fn create_comment(
        &self,
        _issue_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        Ok(IssueComment {
            id: "comment-1".to_string(),
            body: body.to_string(),
            url: None,
        })
    }

    async fn list_comments(&self, _issue_id: &str) -> Result<Vec<IssueComment>, TrackerError> {
        Ok(vec![])
    }

    async fn update_comment(
        &self,
        comment_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        Ok(IssueComment {
            id: comment_id.to_string(),
            body: body.to_string(),
            url: None,
        })
    }

    async fn upload_attachment(
        &self,
        _issue_id: &str,
        path: &Path,
        _content_type: &str,
    ) -> Result<IssueAttachment, TrackerError> {
        Ok(IssueAttachment {
            url: path.display().to_string(),
            comment: None,
        })
    }

    async fn link_pr(&self, _issue_id: &str, _title: &str, _url: &str) -> Result<(), TrackerError> {
        Ok(())
    }
}

fn test_issue(id: &str) -> Issue {
    Issue {
        id: id.to_string(),
        identifier: format!("ISSUE-{id}"),
        title: "Title".to_string(),
        description: None,
        priority: None,
        state: "Todo".to_string(),
        branch_name: None,
        url: None,
        labels: vec![],
        blocked_by: vec![],
        created_at: None,
        updated_at: None,
    }
}

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
fn codex_spawn_command_inserts_model_config_before_app_server_flags() {
    let config = CodexConfig {
        command: "codex   --config shell_environment_policy.inherit=all app-server --stdio"
            .to_string(),
        model: Some("gpt-5.5".to_string()),
        model_reasoning_effort: Some("xhigh".to_string()),
        ..CodexConfig::default()
    };
    assert_eq!(
        codex_spawn_command(&config),
        "codex   --config shell_environment_policy.inherit=all --config 'model=\"gpt-5.5\"' --config 'model_reasoning_effort=xhigh' app-server --stdio"
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
fn codex_spawn_process_command_uses_bash_on_posix() {
    let config = CodexConfig {
        command: "codex --config shell_environment_policy.inherit=all app-server".to_string(),
        model: Some("gpt-5.5".to_string()),
        ..CodexConfig::default()
    };
    let command = codex_spawn_process_command_for_platform(&config, HostPlatform::Posix);
    assert_eq!(command.program(), "bash");
    assert_eq!(
        command.args(),
        &[
            "-lc".to_string(),
            "codex --config shell_environment_policy.inherit=all --config 'model=\"gpt-5.5\"' app-server"
                .to_string()
        ]
    );
}

#[test]
fn codex_spawn_process_command_uses_direct_windows_argv() {
    let config = CodexConfig {
        command: r#"C:\Users\me\bin\codex.exe app-server"#.to_string(),
        model: Some("o'hara".to_string()),
        model_reasoning_effort: Some("xhigh".to_string()),
        ..CodexConfig::default()
    };
    let command = codex_spawn_process_command_for_platform(&config, HostPlatform::Windows);
    assert_eq!(command.program(), r#"C:\Users\me\bin\codex.exe"#);
    assert_eq!(
        command.args(),
        &[
            "--config".to_string(),
            "model=\"o'hara\"".to_string(),
            "--config".to_string(),
            "model_reasoning_effort=xhigh".to_string(),
            "app-server".to_string(),
        ]
    );
}

#[test]
fn codex_spawn_process_command_preserves_quoted_windows_path() {
    let config = CodexConfig {
        command: r#""C:\Program Files\Codex\codex.exe" app-server --stdio"#.to_string(),
        ..CodexConfig::default()
    };
    let command = codex_spawn_process_command_for_platform(&config, HostPlatform::Windows);
    assert_eq!(command.program(), r#"C:\Program Files\Codex\codex.exe"#);
    assert_eq!(
        command.args(),
        &["app-server".to_string(), "--stdio".to_string()]
    );
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
    let tracker: Arc<dyn IssueTracker> = Arc::new(TestTracker);
    let tools = DynamicTools::from_tracker(tracker);
    let payload = thread_start_params(
        Path::new("/tmp/workspace"),
        "VIK-7: optimize workflow codex config",
        &CodexConfig::default(),
        &tools,
    );
    assert_eq!(
        payload.pointer("/dynamicTools/0/name"),
        Some(&json!("vik_issue"))
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

#[test]
fn turn_start_buffered_message_routing_uses_message_turn_id() {
    let early_new_turn = json!({
        "method": "turn/started",
        "params": {
            "threadId": "thread-1",
            "turn": { "id": "turn-2" }
        }
    });
    let stale_old_turn = json!({
        "method": "turn/completed",
        "params": {
            "threadId": "thread-1",
            "turn": { "id": "turn-1" }
        }
    });
    let server_request_without_turn = json!({
        "id": 7,
        "method": "item/tool/call",
        "params": {}
    });

    assert!(message_belongs_to_turn(&early_new_turn, "turn-2"));
    assert!(!message_belongs_to_turn(&stale_old_turn, "turn-2"));
    assert!(message_belongs_to_turn(
        &server_request_without_turn,
        "turn-2"
    ));
}

#[tokio::test]
async fn session_log_appends_raw_codex_jsonl_under_workspace_sessions() {
    let workspace_root = TempDir::new().unwrap();
    let sessions = session_log_dir(workspace_root.path());
    let path = session_log_path(&sessions, "VIK-11", "thread/one:turn two");
    let mut session_log = SessionLog::open(path.clone()).await.unwrap();

    session_log
        .append_message(&json!({
            "method": "turn/started",
            "params": { "turn": { "id": "turn two" } }
        }))
        .await
        .unwrap();
    session_log
        .append_message(&json!({
            "method": "turn/completed",
            "params": { "turn": { "status": "completed" } }
        }))
        .await
        .unwrap();

    assert_eq!(
        path,
        workspace_root
            .path()
            .join("sessions")
            .join("VIK-11-thread_one_turn_two.jsonl")
    );
    let contents = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(!contents.contains("VIK-11"));
    assert!(!contents.contains("thread_one_turn_two"));
    let lines: Vec<_> = contents.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(lines[0]).unwrap()["method"],
        "turn/started"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(lines[1]).unwrap()["method"],
        "turn/completed"
    );
}

#[tokio::test]
async fn session_log_starts_new_line_after_torn_write() {
    let workspace_root = TempDir::new().unwrap();
    let sessions = session_log_dir(workspace_root.path());
    let path = session_log_path(&sessions, "VIK-11", "session-1");
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, b"{\"partial\":true").await.unwrap();
    let mut session_log = SessionLog::open(path.clone()).await.unwrap();

    session_log
        .append_message(&json!({ "method": "turn/completed" }))
        .await
        .unwrap();

    let contents = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(
        contents,
        "{\"partial\":true\n{\"method\":\"turn/completed\"}\n"
    );
}
