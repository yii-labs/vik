use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;
use vik_core::{
    AgentEvent, AgentRunRequest, AgentSession, HostPlatform, Issue, IssueAttachment, IssueComment,
    IssueTracker, IssueUpdate, TrackerError, WorkflowDefinition,
};
use vik_workflow::{
    AgentConfig, AgentRuntimeConfig, CodexConfig, CommonTrackerConfig, HooksConfig,
    LinearTrackerConfig, LoggingConfig, PollingConfig, ServiceConfig, TrackerConfig,
    WorkspaceConfig,
};

use crate::codex::command::{codex_spawn_command, codex_spawn_process_command_for_platform};
use crate::codex::events::extract_usage;
use crate::codex::process::{
    SessionLogContext, permission_approval_result, session_log_fields, thread_start_params,
    turn_start_params,
};
use crate::codex::tools::DynamicTools;
use crate::codex::transport::{CodexTransport, CodexTransportFactory, EventSink};
use crate::codex::{CONTINUATION_PROMPT, Codex, message_belongs_to_turn, session_thread_name};
use crate::{AgentError, AgentRuntime};

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
    let tracker: Arc<dyn IssueTracker> = Arc::new(FakeTracker::new(vec!["Todo"]));
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

#[test]
fn session_log_fields_extract_method_params_and_rpc_results() {
    let request = json!({
        "id": 4,
        "method": "turn/start",
        "params": { "threadId": "thread-1", "input": [] }
    });
    let fields = session_log_fields(&request, None);
    assert_eq!(fields.event, "turn/start");
    assert_eq!(fields.rpc_id.as_deref(), Some("4"));
    assert_eq!(fields.params["threadId"], "thread-1");

    let response = json!({
        "id": 4,
        "result": { "turn": { "id": "turn-1" } }
    });
    let fields = session_log_fields(&response, Some("turn/start"));
    assert_eq!(fields.event, "turn/start");
    assert_eq!(fields.rpc_id.as_deref(), Some("4"));
    assert_eq!(fields.params["turn"]["id"], "turn-1");
}

#[test]
fn session_thread_name_uses_sanitized_issue_identifier() {
    assert_eq!(session_thread_name("VIK-51"), "vik-session-VIK-51");
    assert_eq!(
        session_thread_name("bad issue/51"),
        "vik-session-bad_issue_51"
    );
}

#[tokio::test]
async fn codex_run_uses_issue_prompt_then_continuation_prompt() {
    let dir = TempDir::new().unwrap();
    let tracker = Arc::new(FakeTracker::new(vec!["Todo"]));
    let factory = Arc::new(FakeTransportFactory::default());
    let runtime = Codex::with_transport_factory(Arc::clone(&tracker), factory.clone());
    let (tx, mut rx) = mpsc::unbounded_channel();

    runtime.run(agent_request(dir.path(), 2), tx).await.unwrap();

    let state = factory.state.lock().unwrap();
    assert_eq!(
        state.prompts,
        vec![
            "VIK-1 attempt=1".to_string(),
            CONTINUATION_PROMPT.to_string()
        ]
    );
    assert_eq!(state.unsubscribed_threads, vec!["thread-1"]);
    assert_eq!(state.shutdowns, 1);
    assert_eq!(state.session_contexts_set, 3);
    drop(state);

    let events = drain_events(&mut rx);
    assert!(
        events
            .iter()
            .any(|event| event.event == "codex_process_starting")
    );
    assert!(events.iter().any(|event| event.event == "session_started"));
    assert!(events.iter().any(|event| event.event == "turn/completed"));
}

#[tokio::test]
async fn codex_run_stops_when_issue_leaves_active_states() {
    let dir = TempDir::new().unwrap();
    let tracker = Arc::new(FakeTracker::new(vec!["Done"]));
    let factory = Arc::new(FakeTransportFactory::default());
    let runtime = Codex::with_transport_factory(Arc::clone(&tracker), factory.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    runtime.run(agent_request(dir.path(), 3), tx).await.unwrap();

    let state = factory.state.lock().unwrap();
    assert_eq!(state.prompts, vec!["VIK-1 attempt=1".to_string()]);
    assert_eq!(state.unsubscribed_threads, vec!["thread-1"]);
    assert_eq!(state.shutdowns, 1);
}

#[tokio::test]
async fn codex_run_shuts_down_and_unsubscribes_after_turn_failure() {
    let dir = TempDir::new().unwrap();
    let tracker = Arc::new(FakeTracker::new(vec!["Todo"]));
    let factory = Arc::new(FakeTransportFactory::with_turn_failure());
    let runtime = Codex::with_transport_factory(Arc::clone(&tracker), factory.clone());
    let (tx, _rx) = mpsc::unbounded_channel();

    let err = runtime
        .run(agent_request(dir.path(), 2), tx)
        .await
        .unwrap_err();

    assert!(matches!(err, AgentError::TurnFailed(message) if message == "fake failure"));
    let state = factory.state.lock().unwrap();
    assert_eq!(state.prompts, vec!["VIK-1 attempt=1".to_string()]);
    assert_eq!(state.unsubscribed_threads, vec!["thread-1"]);
    assert_eq!(state.shutdowns, 1);
}

#[tokio::test]
async fn codex_run_abort_cancels_session_thread() {
    let dir = TempDir::new().unwrap();
    let tracker = Arc::new(FakeTracker::new(vec!["Todo"]));
    let factory = Arc::new(FakeTransportFactory::with_blocking_wait());
    let runtime = Codex::with_transport_factory(Arc::clone(&tracker), factory.clone());
    let request = agent_request(dir.path(), 1);
    let (tx, _rx) = mpsc::unbounded_channel();
    let state = Arc::clone(&factory.state);

    let handle = tokio::spawn(async move { runtime.run(request, tx).await });
    wait_until(Duration::from_secs(2), || {
        state.lock().unwrap().waits_started > 0
    })
    .await;

    handle.abort();
    let _ = handle.await;
    wait_until(Duration::from_secs(2), || state.lock().unwrap().drops > 0).await;

    let state = factory.state.lock().unwrap();
    assert_eq!(state.drops, 1);
    assert_eq!(state.wait_thread_names, vec!["vik-session-VIK-1"]);
}

#[tokio::test]
async fn local_agent_worker_maps_runtime_failure_to_worker_outcome() {
    let runtime = Arc::new(FailingRuntime);
    let tracker = Arc::new(FakeTracker::new(vec!["Todo"]));
    let worker = crate::LocalAgentWorker::with_runtime_override(tracker, runtime);
    let (tx, _rx) = mpsc::unbounded_channel();

    let outcome =
        vik_core::AgentWorker::run(&worker, agent_request(Path::new("/tmp"), 1), tx).await;

    assert_eq!(outcome.issue_id, "issue-1");
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("turn_failed"))
    );
}

#[test]
fn local_agent_worker_keeps_tracker_constructor() {
    let tracker = Arc::new(FakeTracker::new(vec!["Todo"]));
    let _worker = crate::LocalAgentWorker::new(tracker);

    // Runtime selection happens for each request from the current workflow config.
}

#[derive(Clone)]
struct FakeTracker {
    states: Arc<Mutex<VecDeque<String>>>,
}

impl FakeTracker {
    fn new(states: Vec<&str>) -> Self {
        Self {
            states: Arc::new(Mutex::new(
                states.into_iter().map(ToOwned::to_owned).collect(),
            )),
        }
    }
}

#[async_trait]
impl IssueTracker for FakeTracker {
    async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError> {
        Ok(Vec::new())
    }

    async fn fetch_by_states(&self, _state_names: &[String]) -> Result<Vec<Issue>, TrackerError> {
        Ok(Vec::new())
    }

    async fn fetch_states_by_ids(&self, _issue_ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        let state = self
            .states
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| "Todo".to_string());
        Ok(vec![issue_with_state(&state)])
    }

    async fn get_issue(&self, issue_id: &str) -> Result<Issue, TrackerError> {
        let mut issue = issue_with_state("Todo");
        issue.id = issue_id.to_string();
        Ok(issue)
    }

    async fn update_issue(
        &self,
        issue_id: &str,
        update: IssueUpdate,
    ) -> Result<Issue, TrackerError> {
        let mut issue = issue_with_state(update.state.as_deref().unwrap_or("Todo"));
        issue.id = issue_id.to_string();
        Ok(issue)
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

    async fn list_comments(&self, issue_id: &str) -> Result<Vec<IssueComment>, TrackerError> {
        Ok(vec![IssueComment {
            id: "comment-1".to_string(),
            body: format!("workpad for {issue_id}"),
            url: None,
        }])
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

#[derive(Default)]
struct FakeTransportState {
    prompts: Vec<String>,
    unsubscribed_threads: Vec<String>,
    shutdowns: usize,
    session_contexts_set: usize,
    fail_turn: bool,
    block_wait: bool,
    waits_started: usize,
    wait_thread_names: Vec<String>,
    drops: usize,
}

#[derive(Default)]
struct FakeTransportFactory {
    state: Arc<Mutex<FakeTransportState>>,
}

impl FakeTransportFactory {
    fn with_turn_failure() -> Self {
        let state = FakeTransportState {
            fail_turn: true,
            ..FakeTransportState::default()
        };
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    fn with_blocking_wait() -> Self {
        let state = FakeTransportState {
            block_wait: true,
            ..FakeTransportState::default()
        };
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }
}

#[async_trait]
impl CodexTransportFactory for FakeTransportFactory {
    async fn spawn(
        &self,
        _command: &crate::codex::process::ProcessCommand,
        _cwd: &Path,
        _config: &CodexConfig,
        _tools: DynamicTools,
    ) -> Result<Box<dyn CodexTransport>, AgentError> {
        Ok(Box::new(FakeTransport {
            state: Arc::clone(&self.state),
        }))
    }
}

struct FakeTransport {
    state: Arc<Mutex<FakeTransportState>>,
}

#[async_trait]
impl CodexTransport for FakeTransport {
    fn process_id(&self) -> Option<String> {
        Some("fake-pid".to_string())
    }

    async fn initialize(&mut self) -> Result<(), AgentError> {
        Ok(())
    }

    async fn thread_start(
        &mut self,
        _cwd: &Path,
        _title: &str,
        _config: &CodexConfig,
    ) -> Result<String, AgentError> {
        Ok("thread-1".to_string())
    }

    async fn turn_start(
        &mut self,
        _thread_id: &str,
        _cwd: &Path,
        prompt: String,
        _config: &CodexConfig,
    ) -> Result<crate::codex::process::TurnStartResponse, AgentError> {
        let mut state = self.state.lock().unwrap();
        state.prompts.push(prompt);
        let turn_id = format!("turn-{}", state.prompts.len());
        Ok(crate::codex::process::TurnStartResponse {
            turn_id: turn_id.clone(),
        })
    }

    fn set_session_log_context(&mut self, _context: SessionLogContext) {
        self.state.lock().unwrap().session_contexts_set += 1;
    }

    async fn wait_for_turn(
        &mut self,
        _thread_id: &str,
        turn_id: &str,
        live: &mut AgentSession,
        issue_id: &str,
        on_event: EventSink<'_>,
    ) -> Result<(), AgentError> {
        if self.state.lock().unwrap().fail_turn {
            return Err(AgentError::TurnFailed("fake failure".to_string()));
        }
        let block_wait = {
            let mut state = self.state.lock().unwrap();
            state.waits_started += 1;
            state.wait_thread_names.push(
                std::thread::current()
                    .name()
                    .unwrap_or_default()
                    .to_string(),
            );
            state.block_wait
        };
        if block_wait {
            std::future::pending::<()>().await;
        }
        live.last_event = Some("turn/completed".to_string());
        on_event(crate::codex::events::agent_event(
            issue_id.to_string(),
            "turn/completed",
            Some(live.clone()),
            None,
            None,
            json!({
                "method": "turn/completed",
                "params": {
                    "threadId": "thread-1",
                    "turn": { "id": turn_id, "status": "completed" }
                }
            }),
        ));
        Ok(())
    }

    async fn unsubscribe(&mut self, thread_id: &str) {
        self.state
            .lock()
            .unwrap()
            .unsubscribed_threads
            .push(thread_id.to_string());
    }

    async fn shutdown(&mut self) {
        self.state.lock().unwrap().shutdowns += 1;
    }
}

impl Drop for FakeTransport {
    fn drop(&mut self) {
        self.state.lock().unwrap().drops += 1;
    }
}

struct FailingRuntime;

#[async_trait]
impl AgentRuntime for FailingRuntime {
    async fn run(
        &self,
        _request: AgentRunRequest<ServiceConfig>,
        _events: mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<(), AgentError> {
        Err(AgentError::TurnFailed("worker failed".to_string()))
    }
}

fn drain_events(rx: &mut mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

async fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < timeout, "condition timed out");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn agent_request(root: &Path, max_turns: u32) -> AgentRunRequest<ServiceConfig> {
    AgentRunRequest {
        issue: issue_with_state("Todo"),
        attempt: Some(1),
        workflow: WorkflowDefinition {
            path: root.join("WORKFLOW.md"),
            config: Default::default(),
            prompt_template: "{{ issue.identifier }} attempt={{ attempt }}".to_string(),
        },
        config: service_config(root, max_turns),
    }
}

fn service_config(root: &Path, max_turns: u32) -> ServiceConfig {
    let workspace_root = root.join("work");
    ServiceConfig {
        workflow_path: root.join("WORKFLOW.md"),
        tracker: TrackerConfig::linear(
            CommonTrackerConfig {
                active_states: vec!["Todo".to_string()],
                terminal_states: vec!["Done".to_string()],
                filter: Default::default(),
            },
            LinearTrackerConfig::new("https://api.linear.app/graphql", "token", "proj"),
        ),
        polling: PollingConfig {
            interval_ms: 30_000,
        },
        workspace: WorkspaceConfig {
            root: workspace_root.clone(),
        },
        logging: LoggingConfig {
            dir: workspace_root.join("logs"),
        },
        hooks: HooksConfig {
            timeout_ms: 60_000,
            ..HooksConfig::default()
        },
        agent: AgentConfig {
            runtime: AgentRuntimeConfig::Codex,
            max_concurrent_agents: 1,
            max_turns,
            max_retry_backoff_ms: 300_000,
            max_concurrent_agents_by_state: Default::default(),
        },
        codex: CodexConfig {
            command: "codex app-server".to_string(),
            turn_timeout_ms: 3_600_000,
            read_timeout_ms: 5_000,
            stall_timeout_ms: 300_000,
            ..CodexConfig::default()
        },
        server: None,
    }
}

fn issue_with_state(state: &str) -> Issue {
    Issue {
        id: "issue-1".to_string(),
        identifier: "VIK-1".to_string(),
        title: "Do work".to_string(),
        description: None,
        priority: Some(1),
        state: state.to_string(),
        branch_name: None,
        url: None,
        labels: Vec::new(),
        blocked_by: Vec::new(),
        created_at: Some(Utc::now()),
        updated_at: None,
    }
}
