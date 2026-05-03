use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::{Issue, WorkflowDefinition, session_id};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    PreparingWorkspace,
    BuildingPrompt,
    LaunchingAgentProcess,
    InitializingSession,
    StreamingTurn,
    Finishing,
    Succeeded,
    Failed,
    TimedOut,
    Stalled,
    CanceledByReconciliation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAttempt {
    pub issue_id: String,
    pub issue_identifier: String,
    pub attempt: Option<u32>,
    pub workspace_path: PathBuf,
    pub started_at: DateTime<Utc>,
    pub status: RunStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub seconds_running: f64,
}

impl Default for TokenTotals {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            seconds_running: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveSession {
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub codex_app_server_pid: Option<String>,
    pub last_codex_event: Option<String>,
    pub last_codex_timestamp: Option<DateTime<Utc>>,
    pub last_codex_message: Option<String>,
    pub codex_input_tokens: u64,
    pub codex_output_tokens: u64,
    pub codex_total_tokens: u64,
    pub last_reported_input_tokens: u64,
    pub last_reported_output_tokens: u64,
    pub last_reported_total_tokens: u64,
    pub turn_count: u32,
}

impl LiveSession {
    pub fn new(thread_id: impl Into<String>, turn_id: impl Into<String>) -> Self {
        let thread_id = thread_id.into();
        let turn_id = turn_id.into();
        Self {
            session_id: session_id(&thread_id, &turn_id),
            thread_id,
            turn_id,
            codex_app_server_pid: None,
            last_codex_event: None,
            last_codex_timestamp: None,
            last_codex_message: None,
            codex_input_tokens: 0,
            codex_output_tokens: 0,
            codex_total_tokens: 0,
            last_reported_input_tokens: 0,
            last_reported_output_tokens: 0,
            last_reported_total_tokens: 0,
            turn_count: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryEntry {
    pub issue_id: String,
    pub identifier: String,
    pub attempt: u32,
    pub due_at: DateTime<Utc>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub issue_id: String,
    #[serde(default)]
    pub session_file_id: String,
    pub event: String,
    pub timestamp: DateTime<Utc>,
    pub codex_app_server_pid: Option<String>,
    pub session: Option<LiveSession>,
    pub usage: Option<TokenUsage>,
    pub rate_limits: Option<Value>,
    pub message: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexSessionLogEntry {
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub session_file_id: String,
    pub issue_id: String,
    pub issue_identifier: String,
    pub source: String,
    pub role: Option<String>,
    pub event: String,
    pub timestamp: DateTime<Utc>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub turn_count: Option<u32>,
    pub codex_app_server_pid: Option<String>,
    pub usage: Option<TokenUsage>,
    pub rate_limits: Option<Value>,
    pub message: Option<String>,
    pub raw: Value,
}

impl CodexSessionLogEntry {
    pub fn from_agent_event(issue_identifier: impl Into<String>, event: &AgentEvent) -> Self {
        let session = event.session.as_ref();
        let issue_identifier = issue_identifier.into();
        let issue_identifier = if issue_identifier.trim().is_empty() {
            event.issue_id.clone()
        } else {
            issue_identifier
        };
        Self {
            sequence: 0,
            session_file_id: String::new(),
            issue_id: event.issue_id.clone(),
            issue_identifier,
            source: "codex_app_server".to_string(),
            role: extract_role(&event.raw),
            event: event.event.clone(),
            timestamp: event.timestamp,
            session_id: session.map(|session| session.session_id.clone()),
            thread_id: session.map(|session| session.thread_id.clone()),
            turn_id: session.map(|session| session.turn_id.clone()),
            turn_count: session.map(|session| session.turn_count),
            codex_app_server_pid: event.codex_app_server_pid.clone(),
            usage: event.usage,
            rate_limits: event.rate_limits.clone(),
            message: event.message.clone(),
            raw: event.raw.clone(),
        }
    }

    pub fn with_session_file_id(mut self, session_file_id: impl Into<String>) -> Self {
        self.session_file_id = session_file_id.into();
        self
    }
}

fn extract_role(raw: &Value) -> Option<String> {
    raw.pointer("/params/message/role")
        .or_else(|| raw.pointer("/params/item/role"))
        .or_else(|| raw.pointer("/params/role"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerExitKind {
    Normal,
    Failed,
    TimedOut,
    Stalled,
    CanceledByReconciliation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerOutcome {
    pub issue_id: String,
    pub issue_identifier: String,
    pub kind: WorkerExitKind,
    pub error: Option<String>,
    pub finished_at: DateTime<Utc>,
}

impl WorkerOutcome {
    pub fn normal(issue: &Issue) -> Self {
        Self {
            issue_id: issue.id.clone(),
            issue_identifier: issue.identifier.clone(),
            kind: WorkerExitKind::Normal,
            error: None,
            finished_at: Utc::now(),
        }
    }

    pub fn failed(issue: &Issue, error: impl Into<String>) -> Self {
        Self {
            issue_id: issue.id.clone(),
            issue_identifier: issue.identifier.clone(),
            kind: WorkerExitKind::Failed,
            error: Some(error.into()),
            finished_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentRunRequest<C> {
    pub issue: Issue,
    pub session_file_id: String,
    pub attempt: Option<u32>,
    pub workflow: WorkflowDefinition,
    pub config: C,
}

#[async_trait]
pub trait AgentWorker<C>: Send + Sync + 'static
where
    C: Send + Sync + Clone + 'static,
{
    async fn run(
        &self,
        request: AgentRunRequest<C>,
        events: mpsc::UnboundedSender<AgentEvent>,
    ) -> WorkerOutcome;
}
