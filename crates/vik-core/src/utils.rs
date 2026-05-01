use crate::{BlockerRef, Issue};

pub fn normalize_state(state: &str) -> String {
    state.to_lowercase()
}

pub fn sanitize_workspace_key(identifier: &str) -> String {
    identifier
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub fn session_id(thread_id: &str, turn_id: &str) -> String {
    format!("{thread_id}-{turn_id}")
}

pub fn issue_is_active(
    issue: &Issue,
    active_states: &[String],
    terminal_states: &[String],
) -> bool {
    let state = normalize_state(&issue.state);
    active_states.iter().any(|s| normalize_state(s) == state)
        && !terminal_states.iter().any(|s| normalize_state(s) == state)
}

pub fn blocker_is_terminal(blocker: &BlockerRef, terminal_states: &[String]) -> bool {
    blocker
        .state
        .as_deref()
        .map(|state| {
            let state = normalize_state(state);
            terminal_states
                .iter()
                .any(|terminal| normalize_state(terminal) == state)
        })
        .unwrap_or(false)
}
