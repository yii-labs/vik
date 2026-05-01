use vik_core::{Issue, blocker_is_terminal, issue_is_active, normalize_state};
use vik_workflow::ServiceConfig;

use crate::BASE_FAILURE_RETRY_MS;
use crate::state::OrchestratorState;

pub fn sort_for_dispatch(mut issues: Vec<Issue>) -> Vec<Issue> {
    issues.sort_by(|a, b| {
        let priority_a = a.priority.unwrap_or(i64::MAX);
        let priority_b = b.priority.unwrap_or(i64::MAX);
        priority_a
            .cmp(&priority_b)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.identifier.cmp(&b.identifier))
    });
    issues
}

pub fn should_dispatch(issue: &Issue, state: &OrchestratorState, config: &ServiceConfig) -> bool {
    if !should_dispatch_retry(issue, state, config) {
        return false;
    }
    !state.claimed.contains(&issue.id)
}

pub fn should_dispatch_retry(
    issue: &Issue,
    state: &OrchestratorState,
    config: &ServiceConfig,
) -> bool {
    if issue.id.is_empty()
        || issue.identifier.is_empty()
        || issue.title.is_empty()
        || issue.state.is_empty()
    {
        return false;
    }
    if !issue_is_active(
        issue,
        &config.tracker.active_states,
        &config.tracker.terminal_states,
    ) {
        return false;
    }
    if state.running.contains_key(&issue.id) {
        return false;
    }
    if available_global_slots(state) == 0 || available_state_slots(issue, state, config) == 0 {
        return false;
    }
    if normalize_state(&issue.state) == "todo"
        && issue
            .blocked_by
            .iter()
            .any(|blocker| !blocker_is_terminal(blocker, &config.tracker.terminal_states))
    {
        return false;
    }
    true
}

pub fn available_global_slots(state: &OrchestratorState) -> usize {
    state
        .max_concurrent_agents
        .saturating_sub(state.running.len())
}

pub fn available_state_slots(
    issue: &Issue,
    state: &OrchestratorState,
    config: &ServiceConfig,
) -> usize {
    let state_name = normalize_state(&issue.state);
    let limit = config
        .agent
        .max_concurrent_agents_by_state
        .get(&state_name)
        .copied()
        .unwrap_or(state.max_concurrent_agents);
    let running_in_state = state
        .running
        .values()
        .filter(|entry| normalize_state(&entry.issue.state) == state_name)
        .count();
    limit.saturating_sub(running_in_state)
}

pub fn failure_backoff_ms(attempt: u32, cap_ms: u64) -> u64 {
    let shift = attempt.saturating_sub(1).min(30);
    let delay = BASE_FAILURE_RETRY_MS.saturating_mul(2_u64.saturating_pow(shift));
    delay.min(cap_ms)
}
