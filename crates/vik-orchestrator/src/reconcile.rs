use std::collections::HashMap;

use chrono::Utc;
use vik_core::{AgentWorker, IssueTracker, normalize_state};
use vik_workflow::ServiceConfig;
use vik_workspace::WorkspaceManager;

use crate::dispatch::failure_backoff_ms;
use crate::engine::Orchestrator;

impl<T, W> Orchestrator<T, W>
where
    T: IssueTracker,
    W: AgentWorker<ServiceConfig>,
{
    pub(crate) async fn reconcile_running_issues(&self) {
        let loaded = self.current_loaded().await;
        self.reconcile_stalled_runs(&loaded.config).await;
        let running_ids: Vec<String> = self.state.lock().await.running.keys().cloned().collect();
        if running_ids.is_empty() {
            return;
        }
        let refreshed = match self.tracker.fetch_states_by_ids(&running_ids).await {
            Ok(issues) => issues,
            Err(err) => {
                tracing::debug!(error=%err, "reconcile_state_refresh outcome=failed keeping_workers=true");
                return;
            }
        };
        let by_id: HashMap<_, _> = refreshed
            .into_iter()
            .map(|issue| (issue.id.clone(), issue))
            .collect();
        for issue_id in running_ids {
            let Some(refreshed_issue) = by_id.get(&issue_id).cloned() else {
                continue;
            };
            let state_name = normalize_state(&refreshed_issue.state);
            let terminal = loaded
                .config
                .tracker
                .terminal_states()
                .iter()
                .any(|state| normalize_state(state) == state_name);
            let active = loaded
                .config
                .tracker
                .active_states()
                .iter()
                .any(|state| normalize_state(state) == state_name);
            if terminal {
                self.terminate_running_issue(&issue_id, true, false, &loaded.config)
                    .await;
            } else if active {
                if let Some(entry) = self.state.lock().await.running.get_mut(&issue_id) {
                    entry.issue = refreshed_issue;
                }
            } else {
                self.terminate_running_issue(&issue_id, false, false, &loaded.config)
                    .await;
            }
        }
    }

    async fn reconcile_stalled_runs(&self, config: &ServiceConfig) {
        if config.codex.stall_timeout_ms <= 0 {
            return;
        }
        let now = Utc::now();
        let stalled: Vec<_> = {
            let state = self.state.lock().await;
            state
                .running
                .iter()
                .filter_map(|(issue_id, entry)| {
                    let since = entry.last_event_at.unwrap_or(entry.started_at);
                    let elapsed = (now - since).num_milliseconds();
                    (elapsed > config.codex.stall_timeout_ms).then_some(issue_id.clone())
                })
                .collect()
        };
        for issue_id in stalled {
            self.terminate_running_issue(&issue_id, false, true, config)
                .await;
            let state = self.state.lock().await;
            if let Some(error) = state.last_errors.get(&issue_id).cloned() {
                tracing::warn!(issue_id=%issue_id, error=%error, "stalled_run outcome=retrying");
            }
        }
    }

    async fn terminate_running_issue(
        &self,
        issue_id: &str,
        cleanup_workspace: bool,
        retry: bool,
        config: &ServiceConfig,
    ) {
        let mut state = self.state.lock().await;
        if let Some(entry) = state.running.remove(issue_id) {
            entry.abort.abort();
            let identifier = entry.identifier.clone();
            if retry {
                let attempt = entry.retry_attempt.unwrap_or(0) + 1;
                let delay = failure_backoff_ms(attempt, config.agent.max_retry_backoff_ms);
                state.schedule_retry(
                    issue_id.to_string(),
                    identifier.clone(),
                    attempt,
                    delay,
                    Some("stalled".to_string()),
                );
            } else {
                state.claimed.remove(issue_id);
            }
            drop(state);
            if cleanup_workspace {
                let manager =
                    WorkspaceManager::new(config.workspace.root.clone(), config.hooks.clone());
                if let Err(err) = manager.remove_for_issue(&identifier).await {
                    tracing::warn!(
                        issue_id=%issue_id,
                        issue_identifier=%identifier,
                        error=%err,
                        "workspace_cleanup outcome=failed"
                    );
                }
            }
        }
    }
}
