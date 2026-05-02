use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;
use vik_core::{
    AttemptSnapshot, CodexSessionLogEntry, IssueDebugSnapshot, RecentEvent, WorkspacePathSnapshot,
    sanitize_workspace_key,
};

const SESSION_LOG_DIR: &str = "codex-session-logs";

pub(crate) fn append_session_log(
    logging_dir: &Path,
    entry: &CodexSessionLogEntry,
) -> io::Result<PathBuf> {
    let path = session_log_path(logging_dir, &entry.issue_identifier);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut entry = entry.clone();
    entry.sequence = next_sequence(&path)?;
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    serde_json::to_writer(&mut file, &entry).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(path)
}

pub(crate) fn read_session_logs(
    logging_dir: &Path,
    issue_identifier: &str,
    limit: usize,
) -> io::Result<Vec<CodexSessionLogEntry>> {
    let path = session_log_path(logging_dir, issue_identifier);
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut entries = VecDeque::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: CodexSessionLogEntry = serde_json::from_str(&line).map_err(io::Error::other)?;
        entries.push_back(entry);
        if limit > 0 && entries.len() > limit {
            entries.pop_front();
        }
    }
    Ok(entries.into_iter().collect())
}

pub(crate) fn attach_session_logs(
    mut snapshot: IssueDebugSnapshot,
    logs: Vec<CodexSessionLogEntry>,
) -> IssueDebugSnapshot {
    if !logs.is_empty() {
        if snapshot.recent_events.is_empty() {
            snapshot.recent_events = recent_events_from_logs(&logs);
        }
        snapshot.session_logs = logs;
    }
    snapshot
}

pub(crate) fn issue_debug_from_session_logs(
    issue_identifier: &str,
    logs: Vec<CodexSessionLogEntry>,
) -> Option<IssueDebugSnapshot> {
    if logs.is_empty() {
        return None;
    }
    let issue_id = logs.last().map(|entry| entry.issue_id.clone());
    Some(IssueDebugSnapshot {
        issue_identifier: issue_identifier.to_string(),
        issue_id,
        status: "persisted".to_string(),
        workspace: None::<WorkspacePathSnapshot>,
        attempts: AttemptSnapshot::default(),
        running: None,
        retry: None,
        recent_events: recent_events_from_logs(&logs),
        session_logs: logs,
        last_error: None,
        tracked: Value::Object(Default::default()),
    })
}

fn recent_events_from_logs(logs: &[CodexSessionLogEntry]) -> Vec<RecentEvent> {
    logs.iter()
        .map(|entry| RecentEvent {
            at: entry.timestamp,
            event: entry.event.clone(),
            message: entry.message.clone(),
        })
        .collect()
}

fn next_sequence(path: &Path) -> io::Result<u64> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(1),
        Err(err) => return Err(err),
    };
    let mut count = 0_u64;
    for line in BufReader::new(file).lines() {
        if !line?.trim().is_empty() {
            count += 1;
        }
    }
    Ok(count + 1)
}

fn session_log_path(logging_dir: &Path, issue_identifier: &str) -> PathBuf {
    logging_dir.join(SESSION_LOG_DIR).join(format!(
        "{}.jsonl",
        sanitize_workspace_key(issue_identifier)
    ))
}
