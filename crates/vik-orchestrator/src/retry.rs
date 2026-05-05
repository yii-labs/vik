use std::collections::HashMap;

use chrono::Utc;
use vik_core::{AgentWorker, IssueTracker, RetryEntry};
use vik_workflow::{LoadedWorkflow, ServiceConfig};

use crate::dispatch::{failure_backoff_ms, should_dispatch_retry};
use crate::engine::Orchestrator;
use crate::gate::DispatchDecision;

impl<T, W> Orchestrator<T, W>
where
    T: IssueTracker,
    W: AgentWorker<ServiceConfig>,
{
    pub(crate) async fn process_due_retries(&self, loaded: LoadedWorkflow) {
        let due: Vec<RetryEntry> = {
            let state = self.state.lock().await;
            let now = Utc::now();
            state
                .retry_attempts
                .values()
                .filter(|entry| entry.due_at <= now)
                .cloned()
                .collect()
        };
        if due.is_empty() {
            return;
        }
        let candidates = match self.tracker.fetch_candidates().await {
            Ok(candidates) => candidates,
            Err(err) => {
                let mut state = self.state.lock().await;
                for entry in due {
                    let attempt = entry.attempt + 1;
                    let delay =
                        failure_backoff_ms(attempt, loaded.config.agent.max_retry_backoff_ms);
                    state.schedule_retry(
                        entry.issue_id,
                        entry.identifier,
                        attempt,
                        delay,
                        Some(format!("retry poll failed: {err}")),
                    );
                }
                return;
            }
        };
        let by_id: HashMap<_, _> = candidates
            .into_iter()
            .map(|issue| (issue.id.clone(), issue))
            .collect();
        for entry in due {
            let mut state = self.state.lock().await;
            state.retry_attempts.remove(&entry.issue_id);
            let Some(issue) = by_id.get(&entry.issue_id).cloned() else {
                state.claimed.remove(&entry.issue_id);
                continue;
            };
            if !should_dispatch_retry(&issue, &state, &loaded.config) {
                let attempt = entry.attempt + 1;
                let delay = failure_backoff_ms(attempt, loaded.config.agent.max_retry_backoff_ms);
                state.schedule_retry(
                    entry.issue_id,
                    issue.identifier,
                    attempt,
                    delay,
                    Some("no available orchestrator slots".to_string()),
                );
                continue;
            }
            drop(state);
            match self.dispatch_decision(&issue).await {
                DispatchDecision::Allow => {
                    self.dispatch_issue(issue, Some(entry.attempt), loaded.clone())
                        .await;
                }
                DispatchDecision::Block(reason) => {
                    tracing::info!(
                        issue_id=%entry.issue_id,
                        issue_identifier=%issue.identifier,
                        attempt=entry.attempt,
                        reason=%reason,
                        "retry_dispatch outcome=gated"
                    );
                }
            }
        }
    }
}
