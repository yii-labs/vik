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
pub struct AgentSession {
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    #[serde(alias = "codex_app_server_pid")]
    pub process_id: Option<String>,
    #[serde(alias = "last_codex_event")]
    pub last_event: Option<String>,
    #[serde(alias = "last_codex_timestamp")]
    pub last_event_at: Option<DateTime<Utc>>,
    #[serde(alias = "last_codex_message")]
    pub last_message: Option<String>,
    #[serde(alias = "codex_input_tokens")]
    pub input_tokens: u64,
    #[serde(alias = "codex_output_tokens")]
    pub output_tokens: u64,
    #[serde(alias = "codex_total_tokens")]
    pub total_tokens: u64,
    pub last_reported_input_tokens: u64,
    pub last_reported_output_tokens: u64,
    pub last_reported_total_tokens: u64,
    pub turn_count: u32,
}

pub type LiveSession = AgentSession;

impl AgentSession {
    pub fn new(thread_id: impl Into<String>, turn_id: impl Into<String>) -> Self {
        let thread_id = thread_id.into();
        let turn_id = turn_id.into();
        Self {
            session_id: session_id(&thread_id, &turn_id),
            thread_id,
            turn_id,
            process_id: None,
            last_event: None,
            last_event_at: None,
            last_message: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
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
    pub event: String,
    pub timestamp: DateTime<Utc>,
    #[serde(alias = "codex_app_server_pid")]
    pub process_id: Option<String>,
    pub session: Option<AgentSession>,
    pub usage: Option<TokenUsage>,
    pub rate_limits: Option<Value>,
    pub message: Option<String>,
    pub raw: Value,
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
