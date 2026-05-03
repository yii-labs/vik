use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::task::JoinHandle;
use vik_core::{
    Issue, RecentEvent, RetryEntry, RunningRow, TokenTotals, TokenUsage, WorkerOutcome,
};
use vik_workflow::ServiceConfig;

#[derive(Debug)]
pub struct OrchestratorState {
    pub poll_interval_ms: u64,
    pub max_concurrent_agents: usize,
    pub running: HashMap<String, RunningEntry>,
    pub issue_identifiers: HashMap<String, String>,
    pub session_log_ids: HashMap<String, String>,
    pub claimed: HashSet<String>,
    pub retry_attempts: HashMap<String, RetryEntry>,
    pub completed: HashSet<String>,
    pub codex_totals: TokenTotals,
    pub codex_rate_limits: Option<Value>,
    pub recent_events: HashMap<String, Vec<RecentEvent>>,
    pub last_errors: HashMap<String, String>,
}

impl OrchestratorState {
    pub fn new(config: &ServiceConfig) -> Self {
        Self {
            poll_interval_ms: config.polling.interval_ms,
            max_concurrent_agents: config.agent.max_concurrent_agents,
            running: HashMap::new(),
            issue_identifiers: HashMap::new(),
            session_log_ids: HashMap::new(),
            claimed: HashSet::new(),
            retry_attempts: HashMap::new(),
            completed: HashSet::new(),
            codex_totals: TokenTotals::default(),
            codex_rate_limits: None,
            recent_events: HashMap::new(),
            last_errors: HashMap::new(),
        }
    }

    pub fn apply_config(&mut self, config: &ServiceConfig) {
        self.poll_interval_ms = config.polling.interval_ms;
        self.max_concurrent_agents = config.agent.max_concurrent_agents;
    }

    pub fn schedule_retry(
        &mut self,
        issue_id: String,
        identifier: String,
        attempt: u32,
        delay_ms: u64,
        error: Option<String>,
    ) {
        self.issue_identifiers
            .insert(issue_id.clone(), identifier.clone());
        self.claimed.insert(issue_id.clone());
        self.retry_attempts.insert(
            issue_id.clone(),
            RetryEntry {
                issue_id,
                identifier,
                attempt,
                due_at: Utc::now() + chrono::Duration::milliseconds(delay_ms as i64),
                error,
            },
        );
    }
}

#[derive(Debug)]
pub struct RunningEntry {
    pub issue: Issue,
    pub identifier: String,
    pub retry_attempt: Option<u32>,
    pub started_at: DateTime<Utc>,
    pub workspace_path: Option<std::path::PathBuf>,
    pub session_id: Option<String>,
    pub turn_count: u32,
    pub last_event: Option<String>,
    pub last_message: Option<String>,
    pub last_event_at: Option<DateTime<Utc>>,
    pub tokens: TokenUsage,
    pub last_reported_input_tokens: u64,
    pub last_reported_output_tokens: u64,
    pub last_reported_total_tokens: u64,
    pub(crate) abort: JoinHandle<WorkerOutcome>,
}

impl RunningEntry {
    pub(crate) fn running_row(&self) -> RunningRow {
        RunningRow {
            issue_id: self.issue.id.clone(),
            issue_identifier: self.issue.identifier.clone(),
            state: self.issue.state.clone(),
            session_id: self.session_id.clone(),
            turn_count: self.turn_count,
            last_event: self.last_event.clone(),
            last_message: self.last_message.clone(),
            started_at: self.started_at,
            last_event_at: self.last_event_at,
            tokens: self.tokens,
            workspace_path: self.workspace_path.clone(),
        }
    }
}
