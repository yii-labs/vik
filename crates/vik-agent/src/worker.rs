use std::sync::Arc;
use std::thread;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use vik_core::{
    AgentEvent, AgentRunRequest, AgentSession, AgentWorker, IssueTracker, WorkerOutcome,
};
use vik_workflow::{AgentRuntimeConfig, ServiceConfig};

use crate::SESSION_LOG_TARGET;
use crate::codex::Codex;
use crate::error::AgentError;
use crate::runtime::AgentRuntime;
use crate::session_log::with_session_log_subscriber;

#[derive(Clone)]
pub struct LocalAgentWorker<T>
where
    T: IssueTracker,
{
    tracker: Arc<T>,
    #[cfg(test)]
    runtime_override: Option<Arc<dyn AgentRuntime>>,
}

impl<T> LocalAgentWorker<T>
where
    T: IssueTracker,
{
    pub fn new(tracker: Arc<T>) -> Self {
        Self {
            tracker,
            #[cfg(test)]
            runtime_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_runtime_override(tracker: Arc<T>, runtime: Arc<dyn AgentRuntime>) -> Self {
        Self {
            tracker,
            runtime_override: Some(runtime),
        }
    }

    fn runtime_for(&self, runtime: AgentRuntimeConfig) -> Arc<dyn AgentRuntime> {
        #[cfg(test)]
        if let Some(runtime) = &self.runtime_override {
            return Arc::clone(runtime);
        }

        match runtime {
            AgentRuntimeConfig::Codex => Arc::new(Codex::new(Arc::clone(&self.tracker))),
        }
    }
}

#[async_trait]
impl<T> AgentWorker<ServiceConfig> for LocalAgentWorker<T>
where
    T: IssueTracker,
{
    async fn run(
        &self,
        request: AgentRunRequest<ServiceConfig>,
        events: mpsc::UnboundedSender<AgentEvent>,
    ) -> WorkerOutcome {
        let issue = request.issue.clone();
        let runtime = self.runtime_for(request.config.agent.runtime);
        match run_runtime_on_session_thread(runtime, request, events).await {
            Ok(()) => WorkerOutcome::normal(&issue),
            Err(err) => WorkerOutcome::failed(&issue, err.to_string()),
        }
    }
}

async fn run_runtime_on_session_thread(
    runtime: Arc<dyn AgentRuntime>,
    request: AgentRunRequest<ServiceConfig>,
    events: mpsc::UnboundedSender<AgentEvent>,
) -> Result<(), AgentError> {
    let thread_name = session_thread_name(&request.issue.identifier);
    let (result_tx, result_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let _cancel_on_drop = SessionCancelOnDrop::new(cancel_tx);
    let handle = thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let result = run_runtime_in_current_thread(runtime, request, events, cancel_rx);
            let _ = result_tx.send(result);
        })
        .map_err(|err| AgentError::SessionThread(err.to_string()))?;
    let result = match result_rx.await {
        Ok(result) => result,
        Err(_) => {
            return match handle.join() {
                Ok(()) => Err(AgentError::SessionThread(
                    "session thread exited without result".into(),
                )),
                Err(_) => Err(AgentError::SessionThread("session thread panicked".into())),
            };
        }
    };
    handle
        .join()
        .map_err(|_| AgentError::SessionThread("session thread panicked".into()))?;
    result
}

fn run_runtime_in_current_thread(
    runtime: Arc<dyn AgentRuntime>,
    request: AgentRunRequest<ServiceConfig>,
    events: mpsc::UnboundedSender<AgentEvent>,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<(), AgentError> {
    let log_dir = request.config.logging.dir.clone();
    let agent = agent_name(request.config.agent.runtime);
    let mut runtime = Some(runtime);
    let mut request = Some(request);
    let mut events = Some(events);
    let mut cancel_rx = Some(cancel_rx);
    match with_session_log_subscriber(&log_dir, || {
        run_runtime_loop(
            runtime.take().expect("runtime is available"),
            request.take().expect("request is available"),
            events.take().expect("events sender is available"),
            cancel_rx.take().expect("cancel receiver is available"),
            agent,
        )
    }) {
        Ok(result) => result,
        Err(err) => {
            tracing::warn!(
                error = %err,
                log_dir = %log_dir.display(),
                "session logging unavailable; continuing without session log capture"
            );
            run_runtime_loop(
                runtime.expect("runtime is available after session log setup failure"),
                request.expect("request is available after session log setup failure"),
                events.expect("events sender is available after session log setup failure"),
                cancel_rx.expect("cancel receiver is available after session log setup failure"),
                agent,
            )
        }
    }
}

fn run_runtime_loop(
    runtime: Arc<dyn AgentRuntime>,
    request: AgentRunRequest<ServiceConfig>,
    events: mpsc::UnboundedSender<AgentEvent>,
    cancel_rx: oneshot::Receiver<()>,
    agent: &'static str,
) -> Result<(), AgentError> {
    let runtime_loop = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| AgentError::SessionThread(err.to_string()))?;
    runtime_loop.block_on(async move {
        let issue_id = request.issue.id.clone();
        let issue_identifier = request.issue.identifier.clone();
        emit_session_record(SessionRecord {
            agent,
            direction: "sent",
            event_name: "run/start",
            issue_id: &issue_id,
            issue_identifier: &issue_identifier,
            session: None,
            process_id: None,
            params: json!({
                "attempt": request.attempt,
                "max_turns": request.config.agent.max_turns,
                "runtime": agent,
                "workflow_path": request.config.workflow_path.display().to_string(),
            }),
        });
        let (logged_events, mut logged_event_rx) = mpsc::unbounded_channel();
        let forward_agent = agent;
        let forward_issue_identifier = issue_identifier.clone();
        let forward_events = tokio::spawn(async move {
            while let Some(event) = logged_event_rx.recv().await {
                emit_agent_event_record(forward_agent, &forward_issue_identifier, &event);
                let _ = events.send(event);
            }
        });
        let result = tokio::select! {
            result = runtime.run(request, logged_events) => result,
            _ = cancel_rx => Err(AgentError::TurnCancelled),
        };
        let _ = forward_events.await;
        emit_run_finish_record(agent, &issue_id, &issue_identifier, &result);
        result
    })
}

struct SessionCancelOnDrop {
    tx: Option<oneshot::Sender<()>>,
}

impl SessionCancelOnDrop {
    fn new(tx: oneshot::Sender<()>) -> Self {
        Self { tx: Some(tx) }
    }
}

impl Drop for SessionCancelOnDrop {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
    }
}

pub(crate) fn session_thread_name(issue_identifier: &str) -> String {
    let sanitized: String = issue_identifier
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let suffix = if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    };
    format!("vik-session-{suffix}")
}

fn agent_name(runtime: AgentRuntimeConfig) -> &'static str {
    match runtime {
        AgentRuntimeConfig::Codex => "codex",
    }
}

fn emit_agent_event_record(agent: &'static str, issue_identifier: &str, event: &AgentEvent) {
    let session = event.session.as_ref();
    emit_session_record(SessionRecord {
        agent,
        direction: "received",
        event_name: &event.event,
        issue_id: &event.issue_id,
        issue_identifier,
        session,
        process_id: event.process_id.as_deref(),
        params: agent_event_params(event),
    });
}

fn emit_run_finish_record(
    agent: &'static str,
    issue_id: &str,
    issue_identifier: &str,
    result: &Result<(), AgentError>,
) {
    let params = match result {
        Ok(()) => json!({ "outcome": "ok" }),
        Err(err) => json!({ "outcome": "error", "error": err.to_string() }),
    };
    emit_session_record(SessionRecord {
        agent,
        direction: "returned",
        event_name: "run/finish",
        issue_id,
        issue_identifier,
        session: None,
        process_id: None,
        params,
    });
}

fn agent_event_params(event: &AgentEvent) -> Value {
    json!({
        "message": event.message,
        "rate_limits": event.rate_limits,
        "raw": event.raw,
        "usage": event.usage,
    })
}

struct SessionRecord<'a> {
    agent: &'static str,
    direction: &'static str,
    event_name: &'a str,
    issue_id: &'a str,
    issue_identifier: &'a str,
    session: Option<&'a AgentSession>,
    process_id: Option<&'a str>,
    params: Value,
}

fn emit_session_record(record: SessionRecord<'_>) {
    let session_id = record
        .session
        .map(|session| session.session_id.as_str())
        .unwrap_or("");
    let thread_id = record
        .session
        .map(|session| session.thread_id.as_str())
        .unwrap_or("");
    let turn_id = record
        .session
        .map(|session| session.turn_id.as_str())
        .unwrap_or("");
    let process_id = record.process_id.unwrap_or("");
    let params_json = record.params.to_string();
    tracing::info!(
        target: SESSION_LOG_TARGET,
        category = "session",
        agent = record.agent,
        direction = record.direction,
        event = record.event_name,
        issue_id = record.issue_id,
        issue_identifier = record.issue_identifier,
        session_id = session_id,
        thread_id = thread_id,
        turn_id = turn_id,
        process_id = process_id,
        params_json = params_json.as_str(),
        "agent_session_message"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::{Value, json};
    use std::path::Path;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use vik_core::{
        Issue, IssueAttachment, IssueComment, IssueUpdate, TokenUsage, TrackerError,
        WorkflowDefinition,
    };
    use vik_workflow::{
        AgentConfig, CodexConfig, CommonTrackerConfig, HooksConfig, LinearTrackerConfig,
        LoggingConfig, PollingConfig, TrackerConfig, WorkspaceConfig,
    };

    #[tokio::test]
    async fn local_agent_worker_writes_session_log_at_worker_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let request = agent_request(dir.path());
        let tracker = Arc::new(FakeTracker);
        let runtime = Arc::new(EventRuntime::new(agent_event()));
        let worker = LocalAgentWorker::with_runtime_override(tracker, runtime);
        let (tx, mut rx) = mpsc::unbounded_channel();

        let outcome = AgentWorker::run(&worker, request, tx).await;

        assert!(matches!(outcome.kind, vik_core::WorkerExitKind::Normal));
        assert_eq!(rx.recv().await.unwrap().event, "turn/completed");
        let records = session_log_records(&dir);
        assert_eq!(records[0]["agent"], "codex");
        assert_eq!(records[0]["event"], "run/start");
        assert_eq!(records[0]["params"]["runtime"], "codex");
        assert_eq!(records[1]["direction"], "received");
        assert_eq!(records[1]["event"], "turn/completed");
        assert_eq!(records[1]["session_id"], "thread-1");
        assert_eq!(records[1]["thread_id"], "thread-1");
        assert_eq!(records[1]["turn_id"], "turn-1");
        assert_eq!(
            records[1]["params"]["raw"]["params"]["turn"]["status"],
            "completed"
        );
        assert_eq!(records[2]["event"], "run/finish");
        assert_eq!(records[2]["params"]["outcome"], "ok");
    }

    #[tokio::test]
    async fn local_agent_worker_runs_when_session_log_setup_fails() {
        let dir = tempfile::tempdir().unwrap();
        let blocked_log_path = dir.path().join("not-a-dir");
        std::fs::write(&blocked_log_path, "file blocks log dir").unwrap();
        let mut request = agent_request(dir.path());
        request.config.logging.dir = blocked_log_path;
        let tracker = Arc::new(FakeTracker);
        let runtime = Arc::new(EventRuntime::new(agent_event()));
        let worker = LocalAgentWorker::with_runtime_override(tracker, runtime);
        let (tx, mut rx) = mpsc::unbounded_channel();

        let outcome = AgentWorker::run(&worker, request, tx).await;

        assert!(matches!(outcome.kind, vik_core::WorkerExitKind::Normal));
        assert_eq!(rx.recv().await.unwrap().event, "turn/completed");
    }

    #[tokio::test]
    async fn local_agent_worker_runs_runtime_on_named_session_thread() {
        let dir = tempfile::tempdir().unwrap();
        let request = agent_request(dir.path());
        let tracker = Arc::new(FakeTracker);
        let runtime = Arc::new(ThreadNameRuntime);
        let worker = LocalAgentWorker::with_runtime_override(tracker, runtime);
        let (tx, mut rx) = mpsc::unbounded_channel();

        let outcome = AgentWorker::run(&worker, request, tx).await;

        assert!(matches!(outcome.kind, vik_core::WorkerExitKind::Normal));
        let event = rx.recv().await.unwrap();
        assert_eq!(event.raw["thread_name"], "vik-session-VIK-1");
    }

    struct EventRuntime {
        event: Mutex<Option<AgentEvent>>,
    }

    impl EventRuntime {
        fn new(event: AgentEvent) -> Self {
            Self {
                event: Mutex::new(Some(event)),
            }
        }
    }

    #[async_trait]
    impl AgentRuntime for EventRuntime {
        async fn run(
            &self,
            _request: AgentRunRequest<ServiceConfig>,
            events: mpsc::UnboundedSender<AgentEvent>,
        ) -> Result<(), AgentError> {
            if let Some(event) = self.event.lock().unwrap().take() {
                let _ = events.send(event);
            }
            Ok(())
        }
    }

    struct ThreadNameRuntime;

    #[async_trait]
    impl AgentRuntime for ThreadNameRuntime {
        async fn run(
            &self,
            _request: AgentRunRequest<ServiceConfig>,
            events: mpsc::UnboundedSender<AgentEvent>,
        ) -> Result<(), AgentError> {
            let thread_name = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_string();
            let _ = events.send(AgentEvent {
                raw: json!({ "thread_name": thread_name }),
                ..agent_event()
            });
            Ok(())
        }
    }

    struct FakeTracker;

    #[async_trait]
    impl IssueTracker for FakeTracker {
        async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError> {
            Ok(Vec::new())
        }

        async fn fetch_by_states(
            &self,
            _state_names: &[String],
        ) -> Result<Vec<Issue>, TrackerError> {
            Ok(Vec::new())
        }

        async fn fetch_states_by_ids(
            &self,
            _issue_ids: &[String],
        ) -> Result<Vec<Issue>, TrackerError> {
            Ok(Vec::new())
        }

        async fn get_issue(&self, _issue_id: &str) -> Result<Issue, TrackerError> {
            Err(TrackerError::UnsupportedTrackerOperation(
                "get_issue".to_string(),
            ))
        }

        async fn update_issue(
            &self,
            _issue_id: &str,
            _update: IssueUpdate,
        ) -> Result<Issue, TrackerError> {
            Err(TrackerError::UnsupportedTrackerOperation(
                "update_issue".to_string(),
            ))
        }

        async fn create_comment(
            &self,
            _issue_id: &str,
            _body: &str,
        ) -> Result<IssueComment, TrackerError> {
            Err(TrackerError::UnsupportedTrackerOperation(
                "create_comment".to_string(),
            ))
        }

        async fn list_comments(&self, _issue_id: &str) -> Result<Vec<IssueComment>, TrackerError> {
            Ok(Vec::new())
        }

        async fn update_comment(
            &self,
            _comment_id: &str,
            _body: &str,
        ) -> Result<IssueComment, TrackerError> {
            Err(TrackerError::UnsupportedTrackerOperation(
                "update_comment".to_string(),
            ))
        }

        async fn upload_attachment(
            &self,
            _issue_id: &str,
            _path: &Path,
            _content_type: &str,
        ) -> Result<IssueAttachment, TrackerError> {
            Err(TrackerError::UnsupportedTrackerOperation(
                "upload_attachment".to_string(),
            ))
        }

        async fn link_pr(
            &self,
            _issue_id: &str,
            _title: &str,
            _url: &str,
        ) -> Result<(), TrackerError> {
            Ok(())
        }
    }

    fn agent_event() -> AgentEvent {
        let live = AgentSession {
            session_id: "thread-1".to_string(),
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
            process_id: Some("123".to_string()),
            last_event: Some("turn/completed".to_string()),
            last_event_at: Some(Utc::now()),
            last_message: Some("done".to_string()),
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: 3,
            last_reported_input_tokens: 1,
            last_reported_output_tokens: 2,
            last_reported_total_tokens: 3,
            turn_count: 1,
        };
        AgentEvent {
            issue_id: "issue-1".to_string(),
            event: "turn/completed".to_string(),
            timestamp: Utc::now(),
            process_id: Some("123".to_string()),
            session: Some(live),
            usage: Some(TokenUsage {
                input_tokens: 1,
                output_tokens: 2,
                total_tokens: 3,
            }),
            rate_limits: None,
            message: Some("done".to_string()),
            raw: json!({
                "method": "turn/completed",
                "params": {
                    "threadId": "thread-1",
                    "turn": { "id": "turn-1", "status": "completed" }
                }
            }),
        }
    }

    fn session_log_records(dir: &TempDir) -> Vec<Value> {
        let log_dir = dir.path().join("work/logs");
        let path = std::fs::read_dir(&log_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.starts_with("session.log"))
            })
            .unwrap();
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn agent_request(root: &Path) -> AgentRunRequest<ServiceConfig> {
        AgentRunRequest {
            issue: issue_with_state("Todo"),
            attempt: Some(1),
            workflow: WorkflowDefinition {
                path: root.join("WORKFLOW.md"),
                config: Default::default(),
                prompt_template: "{{ issue.identifier }}".to_string(),
            },
            config: service_config(root),
        }
    }

    fn service_config(root: &Path) -> ServiceConfig {
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
                max_turns: 20,
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
}
