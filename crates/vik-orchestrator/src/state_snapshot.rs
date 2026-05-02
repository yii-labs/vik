use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::Value;
use vik_core::{
    AttemptSnapshot, IssueDebugSnapshot, RetryRow, RuntimeSnapshot, WorkspacePathSnapshot,
};

use crate::state::OrchestratorState;

impl OrchestratorState {
    pub fn snapshot(&self) -> RuntimeSnapshot {
        let now = Utc::now();
        let running: Vec<_> = self
            .running
            .values()
            .map(|entry| entry.running_row())
            .collect();
        let retrying: Vec<_> = self
            .retry_attempts
            .values()
            .map(|entry| RetryRow {
                issue_id: entry.issue_id.clone(),
                issue_identifier: entry.identifier.clone(),
                attempt: entry.attempt,
                due_at: entry.due_at,
                error: entry.error.clone(),
            })
            .collect();
        let active_seconds: f64 = self
            .running
            .values()
            .map(|entry| (now - entry.started_at).num_milliseconds().max(0) as f64 / 1000.0)
            .sum();
        let mut totals = self.codex_totals.clone();
        totals.seconds_running += active_seconds;
        RuntimeSnapshot {
            generated_at: now,
            counts: BTreeMap::from([
                ("running".to_string(), running.len()),
                ("retrying".to_string(), retrying.len()),
            ]),
            running,
            retrying,
            codex_totals: totals,
            rate_limits: self.codex_rate_limits.clone(),
        }
    }

    pub fn issue_debug(&self, issue_identifier: &str) -> Option<IssueDebugSnapshot> {
        let running = self
            .running
            .values()
            .find(|entry| entry.issue.identifier == issue_identifier)
            .map(|entry| entry.running_row());
        let retry = self
            .retry_attempts
            .values()
            .find(|entry| entry.identifier == issue_identifier)
            .map(|entry| RetryRow {
                issue_id: entry.issue_id.clone(),
                issue_identifier: entry.identifier.clone(),
                attempt: entry.attempt,
                due_at: entry.due_at,
                error: entry.error.clone(),
            });
        if running.is_none() && retry.is_none() {
            return None;
        }
        let issue_id = running
            .as_ref()
            .map(|row| row.issue_id.clone())
            .or_else(|| retry.as_ref().map(|row| row.issue_id.clone()));
        let recent_events = issue_id
            .as_ref()
            .and_then(|id| self.recent_events.get(id))
            .cloned()
            .unwrap_or_default();
        let last_error = issue_id
            .as_ref()
            .and_then(|id| self.last_errors.get(id))
            .cloned();
        Some(IssueDebugSnapshot {
            issue_identifier: issue_identifier.to_string(),
            issue_id,
            status: if running.is_some() {
                "running"
            } else {
                "retrying"
            }
            .to_string(),
            workspace: running
                .as_ref()
                .and_then(|row| row.workspace_path.clone())
                .map(|path| WorkspacePathSnapshot { path }),
            attempts: AttemptSnapshot {
                restart_count: retry.as_ref().map(|row| row.attempt).unwrap_or(0),
                current_retry_attempt: retry.as_ref().map(|row| row.attempt),
            },
            running,
            retry,
            recent_events,
            session_logs: Vec::new(),
            last_error,
            tracked: Value::Object(Default::default()),
        })
    }
}
