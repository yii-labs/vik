use std::collections::HashMap;
use std::io::Write;

use chrono::{TimeZone, Utc};
use serde_json::json;
use vik_core::{AgentEvent, BlockerRef, Issue, TokenUsage, WorkerOutcome};
use vik_workflow::{
    AgentConfig, CodexConfig, HooksConfig, LoggingConfig, PollingConfig, ServiceConfig,
    TrackerConfig, WorkspaceConfig,
};

use crate::session_log::{append_session_log, issue_debug_from_session_logs, read_session_logs};
use crate::{
    OrchestratorState, RunningEntry, failure_backoff_ms, should_dispatch, sort_for_dispatch,
};

fn config() -> ServiceConfig {
    ServiceConfig {
        workflow_path: "WORKFLOW.md".into(),
        tracker: TrackerConfig {
            kind: "linear".into(),
            endpoint: "https://api.linear.app/graphql".into(),
            api_key: "token".into(),
            project_slug: "proj".into(),
            active_states: vec!["Todo".into(), "In Progress".into()],
            terminal_states: vec!["Done".into(), "Closed".into()],
        },
        polling: PollingConfig {
            interval_ms: 30_000,
        },
        workspace: WorkspaceConfig {
            root: "/tmp/vik".into(),
        },
        logging: LoggingConfig {
            dir: "/tmp/vik/.vik/logs".into(),
        },
        hooks: HooksConfig {
            timeout_ms: 60_000,
            ..HooksConfig::default()
        },
        agent: AgentConfig {
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
            session_file_id: "20260503T000000Z-aaa".into(),
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
            session_file_id: "20260503T000000Z-bbb".into(),
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
        session_file_id: None,
        event: "codex_thread_starting".into(),
        timestamp: Utc::now(),
        codex_app_server_pid: None,
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

#[tokio::test]
async fn codex_session_log_persists_for_reopen() {
    let config = config();
    let mut state = OrchestratorState::new(&config);
    let current_issue = issue("A", Some(1), 1, "Todo");
    let handle = tokio::spawn(async { WorkerOutcome::normal(&issue("A", Some(1), 1, "Todo")) });
    state.running.insert(
        "A".into(),
        RunningEntry {
            issue: current_issue,
            identifier: "VIK-11".into(),
            retry_attempt: None,
            started_at: Utc::now(),
            workspace_path: None,
            session_file_id: "20260503T000000Z-ccc".into(),
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

    let entry = state.apply_agent_event(AgentEvent {
        issue_id: "A".into(),
        session_file_id: None,
        event: "item/completed".into(),
        timestamp: Utc::now(),
        codex_app_server_pid: Some("123".into()),
        session: Some(vik_core::LiveSession::new("thread-1", "turn-1")),
        usage: Some(TokenUsage {
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: 3,
        }),
        rate_limits: None,
        message: Some("done".into()),
        raw: json!({
            "method": "item/completed",
            "params": {
                "message": {
                    "role": "assistant",
                    "text": "done"
                }
            }
        }),
    });
    let dir = tempfile::tempdir().unwrap();
    let path = append_session_log(dir.path(), &entry).unwrap();
    assert!(path.ends_with("sessions/VIK-11-20260503T000000Z-ccc.jsonl"));

    let reloaded = read_session_logs(dir.path(), "VIK-11", 50).unwrap();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].sequence, 1);
    assert_eq!(reloaded[0].session_file_id, "20260503T000000Z-ccc");
    assert_eq!(reloaded[0].issue_identifier, "VIK-11");
    assert_eq!(reloaded[0].source, "codex_app_server");
    assert_eq!(reloaded[0].role.as_deref(), Some("assistant"));
    assert_eq!(reloaded[0].session_id.as_deref(), Some("thread-1-turn-1"));
    assert_eq!(reloaded[0].message.as_deref(), Some("done"));
    assert_eq!(reloaded[0].usage.unwrap().total_tokens, 3);
    assert_eq!(
        reloaded[0].raw.pointer("/params/message/text"),
        Some(&json!("done"))
    );
}

#[tokio::test]
async fn late_agent_event_uses_known_issue_identifier_after_worker_exit() {
    let config = config();
    let mut state = OrchestratorState::new(&config);
    let mut current_issue = issue("A", Some(1), 1, "Todo");
    current_issue.identifier = "VIK-11".into();
    let handle = tokio::spawn(async { WorkerOutcome::normal(&issue("A", Some(1), 1, "Todo")) });
    state.running.insert(
        "A".into(),
        RunningEntry {
            issue: current_issue.clone(),
            identifier: current_issue.identifier.clone(),
            retry_attempt: None,
            started_at: Utc::now(),
            workspace_path: None,
            session_file_id: "20260503T000000Z-ddd".into(),
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

    let entry = state.apply_agent_event(AgentEvent {
        issue_id: "A".into(),
        session_file_id: None,
        event: "item/completed".into(),
        timestamp: Utc::now(),
        codex_app_server_pid: None,
        session: Some(vik_core::LiveSession::new("thread-1", "turn-1")),
        usage: None,
        rate_limits: None,
        message: Some("tail event".into()),
        raw: json!({ "method": "item/completed" }),
    });

    assert_eq!(entry.issue_identifier, "VIK-11");
    assert_eq!(entry.session_file_id, "20260503T000000Z-ddd");
}

#[tokio::test]
async fn queued_agent_event_keeps_originating_session_file_id_after_redispatch() {
    let config = config();
    let mut state = OrchestratorState::new(&config);
    let mut current_issue = issue("A", Some(1), 1, "Todo");
    current_issue.identifier = "VIK-11".into();
    let handle = tokio::spawn(async { WorkerOutcome::normal(&issue("A", Some(1), 1, "Todo")) });
    state.running.insert(
        "A".into(),
        RunningEntry {
            issue: current_issue,
            identifier: "VIK-11".into(),
            retry_attempt: Some(1),
            started_at: Utc::now(),
            workspace_path: None,
            session_file_id: "20260503T000100Z-new".into(),
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
    state
        .session_file_ids
        .insert("A".into(), "20260503T000100Z-new".into());

    let entry = state.apply_agent_event(AgentEvent {
        issue_id: "A".into(),
        session_file_id: Some("20260503T000000Z-old".into()),
        event: "item/completed".into(),
        timestamp: Utc::now(),
        codex_app_server_pid: None,
        session: Some(vik_core::LiveSession::new("thread-1", "turn-1")),
        usage: None,
        rate_limits: None,
        message: Some("old queued event".into()),
        raw: json!({ "method": "item/completed" }),
    });

    assert_eq!(entry.session_file_id, "20260503T000000Z-old");
    assert_eq!(
        state.session_file_ids["A"],
        "20260503T000100Z-new",
        "stale events must not replace the active run file id"
    );
    assert!(state.running["A"].last_event.is_none());
}

#[test]
fn session_log_read_skips_malformed_lines_and_continues_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let event = AgentEvent {
        issue_id: "A".into(),
        session_file_id: None,
        event: "turn/completed".into(),
        timestamp: Utc::now(),
        codex_app_server_pid: None,
        session: Some(vik_core::LiveSession::new("thread-1", "turn-1")),
        usage: None,
        rate_limits: None,
        message: Some("first".into()),
        raw: json!({ "method": "turn/completed" }),
    };
    let mut first = vik_core::CodexSessionLogEntry::from_agent_event("VIK-11", &event)
        .with_session_file_id("20260503T000000Z-eee");
    first.message = Some("first".into());
    let path = append_session_log(dir.path(), &first).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    write!(file, "{{not json").unwrap();
    drop(file);

    let mut second = first.clone();
    second.message = Some("second".into());
    append_session_log(dir.path(), &second).unwrap();

    let reloaded = read_session_logs(dir.path(), "VIK-11", 50).unwrap();
    assert_eq!(reloaded.len(), 2);
    assert_eq!(reloaded[0].sequence, 1);
    assert_eq!(reloaded[0].message.as_deref(), Some("first"));
    assert_eq!(reloaded[1].sequence, 2);
    assert_eq!(reloaded[1].message.as_deref(), Some("second"));
}

#[test]
fn session_log_read_combines_separate_issue_context_files_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let first_event = AgentEvent {
        issue_id: "A".into(),
        session_file_id: None,
        event: "turn/started".into(),
        timestamp: Utc.with_ymd_and_hms(2026, 5, 3, 1, 0, 0).unwrap(),
        codex_app_server_pid: None,
        session: Some(vik_core::LiveSession::new("thread-1", "turn-1")),
        usage: None,
        rate_limits: None,
        message: Some("first context".into()),
        raw: json!({ "method": "turn/started" }),
    };
    let second_event = AgentEvent {
        issue_id: "A".into(),
        session_file_id: None,
        event: "turn/started".into(),
        timestamp: Utc.with_ymd_and_hms(2026, 5, 3, 2, 0, 0).unwrap(),
        codex_app_server_pid: None,
        session: Some(vik_core::LiveSession::new("thread-2", "turn-1")),
        usage: None,
        rate_limits: None,
        message: Some("second context".into()),
        raw: json!({ "method": "turn/started" }),
    };
    let first = vik_core::CodexSessionLogEntry::from_agent_event("VIK-11", &first_event)
        .with_session_file_id("20260503T010000Z-aaa");
    let second = vik_core::CodexSessionLogEntry::from_agent_event("VIK-11", &second_event)
        .with_session_file_id("20260503T020000Z-bbb");

    let first_path = append_session_log(dir.path(), &first).unwrap();
    let second_path = append_session_log(dir.path(), &second).unwrap();
    assert!(first_path.ends_with("sessions/VIK-11-20260503T010000Z-aaa.jsonl"));
    assert!(second_path.ends_with("sessions/VIK-11-20260503T020000Z-bbb.jsonl"));

    let reloaded = read_session_logs(dir.path(), "VIK-11", 50).unwrap();
    assert_eq!(reloaded.len(), 2);
    assert_eq!(reloaded[0].session_file_id, "20260503T010000Z-aaa");
    assert_eq!(reloaded[1].session_file_id, "20260503T020000Z-bbb");

    let latest = read_session_logs(dir.path(), "VIK-11", 1).unwrap();
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].message.as_deref(), Some("second context"));
}

#[test]
fn issue_debug_can_be_rebuilt_from_persisted_session_logs() {
    let event = AgentEvent {
        issue_id: "A".into(),
        session_file_id: None,
        event: "turn/completed".into(),
        timestamp: Utc::now(),
        codex_app_server_pid: None,
        session: Some(vik_core::LiveSession::new("thread-1", "turn-1")),
        usage: None,
        rate_limits: None,
        message: Some("complete".into()),
        raw: json!({
            "method": "turn/completed",
            "params": {
                "turn": {
                    "id": "turn-1",
                    "status": "completed"
                }
            }
        }),
    };
    let logs = vec![vik_core::CodexSessionLogEntry::from_agent_event(
        "VIK-11", &event,
    )];

    let snapshot = issue_debug_from_session_logs("VIK-11", logs.clone()).unwrap();
    assert_eq!(snapshot.status, "persisted");
    assert_eq!(snapshot.issue_identifier, "VIK-11");
    assert_eq!(snapshot.issue_id.as_deref(), Some("A"));
    assert_eq!(snapshot.recent_events[0].event, "turn/completed");
    assert_eq!(snapshot.session_logs, logs);
}
