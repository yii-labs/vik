use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{RetryEntry, TokenTotals, TokenUsage};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeStateView {
    pub poll_interval_ms: u64,
    pub max_concurrent_agents: usize,
    pub running_issue_ids: HashSet<String>,
    pub claimed: HashSet<String>,
    pub retry_attempts: HashMap<String, RetryEntry>,
    pub completed: HashSet<String>,
    pub token_totals: TokenTotals,
    pub rate_limits: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunningRow {
    pub issue_id: String,
    pub issue_identifier: String,
    pub state: String,
    pub session_id: Option<String>,
    pub turn_count: u32,
    pub last_event: Option<String>,
    pub last_message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_event_at: Option<DateTime<Utc>>,
    pub tokens: TokenUsage,
    pub workspace_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryRow {
    pub issue_id: String,
    pub issue_identifier: String,
    pub attempt: u32,
    pub due_at: DateTime<Utc>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub generated_at: DateTime<Utc>,
    pub counts: BTreeMap<String, usize>,
    pub running: Vec<RunningRow>,
    pub retrying: Vec<RetryRow>,
    pub token_totals: TokenTotals,
    pub rate_limits: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueDebugSnapshot {
    pub issue_identifier: String,
    pub issue_id: Option<String>,
    pub status: String,
    pub workspace: Option<WorkspacePathSnapshot>,
    pub attempts: AttemptSnapshot,
    pub running: Option<RunningRow>,
    pub retry: Option<RetryRow>,
    pub recent_events: Vec<RecentEvent>,
    pub last_error: Option<String>,
    pub tracked: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspacePathSnapshot {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptSnapshot {
    pub restart_count: u32,
    pub current_retry_attempt: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEvent {
    pub at: DateTime<Utc>,
    pub event: String,
    pub message: Option<String>,
}
