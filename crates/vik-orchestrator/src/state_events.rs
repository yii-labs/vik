use vik_core::{AgentEvent, CodexSessionLogEntry, RecentEvent, WorkerExitKind, WorkerOutcome};
use vik_workflow::ServiceConfig;

use crate::CONTINUATION_RETRY_MS;
use crate::dispatch::failure_backoff_ms;
use crate::state::OrchestratorState;

impl OrchestratorState {
    pub fn apply_agent_event(&mut self, event: AgentEvent) -> CodexSessionLogEntry {
        let issue_identifier = self
            .running
            .get(&event.issue_id)
            .map(|entry| entry.identifier.clone())
            .or_else(|| self.issue_identifiers.get(&event.issue_id).cloned())
            .unwrap_or_default();
        if !issue_identifier.is_empty() {
            self.issue_identifiers
                .insert(event.issue_id.clone(), issue_identifier.clone());
        }
        let session_id = event
            .session
            .as_ref()
            .map(|session| session.session_id.clone())
            .unwrap_or_default();
        tracing::info!(
            issue_id=%event.issue_id,
            issue_identifier,
            session_id,
            codex_event=%event.event,
            "codex_update outcome=received"
        );
        let log_entry = CodexSessionLogEntry::from_agent_event(issue_identifier, &event);
        if let Some(entry) = self.running.get_mut(&event.issue_id) {
            entry.last_event = Some(event.event.clone());
            entry.last_message = event.message.clone();
            entry.last_event_at = Some(event.timestamp);
            if let Some(usage) = event.usage {
                let input_delta = usage
                    .input_tokens
                    .saturating_sub(entry.last_reported_input_tokens);
                let output_delta = usage
                    .output_tokens
                    .saturating_sub(entry.last_reported_output_tokens);
                let total_delta = usage
                    .total_tokens
                    .saturating_sub(entry.last_reported_total_tokens);
                self.codex_totals.input_tokens += input_delta;
                self.codex_totals.output_tokens += output_delta;
                self.codex_totals.total_tokens += total_delta;
                entry.last_reported_input_tokens = usage.input_tokens;
                entry.last_reported_output_tokens = usage.output_tokens;
                entry.last_reported_total_tokens = usage.total_tokens;
                entry.tokens = usage;
            }
            if let Some(session) = event.session.clone() {
                entry.session_id = Some(session.session_id);
                entry.turn_count = session.turn_count;
            }
        }
        if let Some(rate_limits) = event.rate_limits {
            self.codex_rate_limits = Some(rate_limits);
        }
        self.recent_events
            .entry(event.issue_id.clone())
            .or_default()
            .push(RecentEvent {
                at: event.timestamp,
                event: event.event,
                message: event.message,
            });
        if let Some(events) = self.recent_events.get_mut(&event.issue_id)
            && events.len() > 50
        {
            events.drain(0..events.len() - 50);
        }
        log_entry
    }

    pub fn on_worker_exit(&mut self, outcome: WorkerOutcome, config: &ServiceConfig) {
        tracing::info!(
            issue_id=%outcome.issue_id,
            issue_identifier=%outcome.issue_identifier,
            exit_kind=?outcome.kind,
            error=?outcome.error,
            "worker_exit outcome=received"
        );
        self.issue_identifiers
            .insert(outcome.issue_id.clone(), outcome.issue_identifier.clone());
        let running = self.running.remove(&outcome.issue_id);
        if let Some(entry) = running {
            self.codex_totals.seconds_running += (outcome.finished_at - entry.started_at)
                .num_milliseconds()
                .max(0) as f64
                / 1000.0;
            match outcome.kind {
                WorkerExitKind::Normal => {
                    self.completed.insert(outcome.issue_id.clone());
                    self.schedule_retry(
                        outcome.issue_id,
                        outcome.issue_identifier,
                        1,
                        CONTINUATION_RETRY_MS,
                        None,
                    );
                }
                _ => {
                    let attempt = entry.retry_attempt.unwrap_or(0) + 1;
                    let delay = failure_backoff_ms(attempt, config.agent.max_retry_backoff_ms);
                    self.last_errors.insert(
                        outcome.issue_id.clone(),
                        outcome
                            .error
                            .clone()
                            .unwrap_or_else(|| "worker failed".to_string()),
                    );
                    self.schedule_retry(
                        outcome.issue_id,
                        outcome.issue_identifier,
                        attempt,
                        delay,
                        outcome.error,
                    );
                }
            }
        }
    }
}
