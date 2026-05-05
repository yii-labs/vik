use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use serde_json::json;
use vik_core::{AgentEvent, BlockerRef, Issue, TokenUsage, WorkerOutcome};
use vik_workflow::{
    AgentConfig, AgentRuntimeConfig, CodexConfig, CommonTrackerConfig, HooksConfig,
    LinearTrackerConfig, LoggingConfig, PollingConfig, ServiceConfig, TrackerConfig,
    WorkspaceConfig,
};

use crate::state_events::should_log_agent_event_to_service;
use crate::{
    OrchestratorState, RunningEntry, failure_backoff_ms, should_dispatch, sort_for_dispatch,
};

fn config() -> ServiceConfig {
    ServiceConfig {
        workflow_path: "WORKFLOW.md".into(),
        tracker: TrackerConfig::linear(
            CommonTrackerConfig {
                active_states: vec!["Todo".into(), "In Progress".into()],
                terminal_states: vec!["Done".into(), "Closed".into()],
                filter: Default::default(),
            },
            LinearTrackerConfig::new("https://api.linear.app/graphql", "token", "proj"),
        ),
        polling: PollingConfig {
            interval_ms: 30_000,
        },
        workspace: WorkspaceConfig {
            root: "/tmp/vik".into(),
        },
        logging: LoggingConfig {
            dir: "/tmp/vik/logs".into(),
        },
        hooks: HooksConfig {
            timeout_ms: 60_000,
            ..HooksConfig::default()
        },
        agent: AgentConfig {
            runtime: AgentRuntimeConfig::Codex,
            max_concurrent_agents: 2,
            max_turns: 20,
            max_retry_backoff_ms: 300_000,
            max_concurrent_agents_by_state: HashMap::new(),
        },
        codex: CodexConfig {
            command: "codex app-server".into(),
            turn_timeout_ms: 3_600_000,
            read_timeout_ms: 5_000,
            stall_timeout_ms: 300_000,
            ..CodexConfig::default()
        },
        server: None,
    }
}

fn issue(id: &str, priority: Option<i64>, created_day: u32, state: &str) -> Issue {
    Issue {
        id: id.into(),
        identifier: id.into(),
        title: "Title".into(),
        description: None,
        priority,
        state: state.into(),
        branch_name: None,
        url: None,
        labels: vec![],
        blocked_by: vec![],
        created_at: Some(Utc.with_ymd_and_hms(2026, 1, created_day, 0, 0, 0).unwrap()),
        updated_at: None,
    }
}

#[test]
fn sorts_by_priority_then_created_then_identifier() {
    let sorted = sort_for_dispatch(vec![
        issue("B", Some(2), 1, "Todo"),
        issue("A", Some(1), 2, "Todo"),
        issue("C", Some(1), 1, "Todo"),
    ]);
    let ids: Vec<_> = sorted.into_iter().map(|issue| issue.id).collect();
    assert_eq!(ids, vec!["C", "A", "B"]);
}

#[test]
fn todo_with_non_terminal_blocker_is_not_eligible() {
    let config = config();
    let state = OrchestratorState::new(&config);
    let mut blocked = issue("A", Some(1), 1, "Todo");
    blocked.blocked_by.push(BlockerRef {
        id: Some("B".into()),
        identifier: Some("B".into()),
        state: Some("Todo".into()),
    });
    assert!(!should_dispatch(&blocked, &state, &config));
    blocked.blocked_by[0].state = Some("Done".into());
    assert!(should_dispatch(&blocked, &state, &config));
}

#[test]
fn backoff_uses_cap() {
    assert_eq!(failure_backoff_ms(1, 300_000), 10_000);
    assert_eq!(failure_backoff_ms(2, 300_000), 20_000);
    assert_eq!(failure_backoff_ms(10, 30_000), 30_000);
}

#[tokio::test]
async fn normal_exit_schedules_continuation_retry() {
    let config = config();
    let mut state = OrchestratorState::new(&config);
    let current_issue = issue("A", Some(1), 1, "Todo");
    let handle = tokio::spawn(async { WorkerOutcome::normal(&issue("A", Some(1), 1, "Todo")) });
    state.running.insert(
        "A".into(),
        RunningEntry {
            issue: current_issue.clone(),
            identifier: current_issue.identifier.clone(),
            retry_attempt: None,
            started_at: Utc::now(),
            workspace_path: None,
            session_id: None,
            turn_count: 0,
            last_event: None,
            last_message: None,
            last_event_at: None,
            tokens: TokenUsage::default(),
            last_reported_input_tokens: 0,
            last_reported_output_tokens: 0,
            last_reported_total_tokens: 0,
            abort: handle,
        },
    );
    state.on_worker_exit(WorkerOutcome::normal(&current_issue), &config);
    assert_eq!(state.retry_attempts["A"].attempt, 1);
}

#[tokio::test]
async fn lifecycle_event_without_session_updates_running_status() {
    let config = config();
    let mut state = OrchestratorState::new(&config);
    let current_issue = issue("A", Some(1), 1, "Todo");
    let handle = tokio::spawn(async { WorkerOutcome::normal(&issue("A", Some(1), 1, "Todo")) });
    state.running.insert(
        "A".into(),
        RunningEntry {
            issue: current_issue.clone(),
            identifier: current_issue.identifier.clone(),
            retry_attempt: None,
            started_at: Utc::now(),
            workspace_path: None,
            session_id: None,
            turn_count: 0,
            last_event: None,
            last_message: None,
            last_event_at: None,
            tokens: TokenUsage::default(),
            last_reported_input_tokens: 0,
            last_reported_output_tokens: 0,
            last_reported_total_tokens: 0,
            abort: handle,
        },
    );

    state.apply_agent_event(AgentEvent {
        issue_id: "A".into(),
        event: "codex_thread_starting".into(),
        timestamp: Utc::now(),
        process_id: None,
        session: None,
        usage: None,
        rate_limits: None,
        message: Some("starting".into()),
        raw: json!({ "cwd": "/tmp/vik/A" }),
    });

    let running = state.running.get("A").unwrap();
    assert_eq!(running.last_event.as_deref(), Some("codex_thread_starting"));
    assert_eq!(running.last_message.as_deref(), Some("starting"));
    assert!(running.last_event_at.is_some());
    assert_eq!(state.recent_events["A"][0].event, "codex_thread_starting");
}

#[test]
fn service_log_decision_suppresses_session_log_duplicates() {
    let agent_event = AgentEvent {
        issue_id: "A".into(),
        event: "turn/completed".into(),
        timestamp: Utc::now(),
        process_id: Some("123".into()),
        session: Some(vik_core::AgentSession::new("thread-1", "turn-1")),
        usage: None,
        rate_limits: None,
        message: Some("completed".into()),
        raw: json!({ "method": "turn/completed" }),
    };
    assert!(!should_log_agent_event_to_service(&agent_event));

    let mut session_started = agent_event.clone();
    session_started.event = "session_started".into();
    assert!(should_log_agent_event_to_service(&session_started));

    let mut lifecycle = agent_event.clone();
    lifecycle.event = "codex_thread_starting".into();
    lifecycle.session = None;
    assert!(should_log_agent_event_to_service(&lifecycle));
}
