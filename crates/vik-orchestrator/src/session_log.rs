use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::task;
use vik_core::{
    AttemptSnapshot, CodexSessionLogEntry, IssueDebugSnapshot, RecentEvent, WorkspacePathSnapshot,
    sanitize_workspace_key,
};

const SESSION_LOG_DIR: &str = "sessions";
const SESSION_LOG_TAIL_CHUNK_BYTES: u64 = 16 * 1024;

pub(crate) async fn append_session_log_blocking(
    logging_dir: PathBuf,
    entry: CodexSessionLogEntry,
) -> io::Result<PathBuf> {
    task::spawn_blocking(move || append_session_log(&logging_dir, &entry))
        .await
        .map_err(|err| io::Error::other(format!("session log append task failed: {err}")))?
}

pub(crate) async fn read_session_logs_blocking(
    logging_dir: PathBuf,
    issue_identifier: String,
    limit: usize,
) -> io::Result<Vec<CodexSessionLogEntry>> {
    task::spawn_blocking(move || read_session_logs(&logging_dir, &issue_identifier, limit))
        .await
        .map_err(|err| io::Error::other(format!("session log read task failed: {err}")))?
}

pub(crate) fn append_session_log(
    logging_dir: &Path,
    entry: &CodexSessionLogEntry,
) -> io::Result<PathBuf> {
    let path = session_log_path(logging_dir, entry);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut entry = entry.clone();
    entry.sequence = next_sequence(&path)?;
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)?;
    finish_torn_jsonl_line(&mut file)?;
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
    let paths = session_log_paths(logging_dir, issue_identifier)?;
    let mut entries = Vec::new();
    for path in paths {
        let mut file_entries = if limit > 0 {
            read_recent_session_logs(&path, limit)?
        } else {
            read_all_session_logs(&path)?
        };
        entries.append(&mut file_entries);
    }
    entries.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.session_file_id.cmp(&right.session_file_id))
            .then_with(|| left.sequence.cmp(&right.sequence))
    });
    if limit > 0 && entries.len() > limit {
        entries.drain(0..entries.len() - limit);
    }
    Ok(entries)
}

fn read_all_session_logs(path: &Path) -> io::Result<Vec<CodexSessionLogEntry>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if let Some(entry) = parse_session_log_line(&line) {
            entries.push(entry);
        }
    }
    Ok(entries)
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
    Ok(last_sequence(path)?.unwrap_or_default() + 1)
}

fn finish_torn_jsonl_line(file: &mut File) -> io::Result<()> {
    if file.metadata()?.len() == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)?;
    if last[0] != b'\n' {
        file.write_all(b"\n")?;
    }
    Ok(())
}

fn session_log_path(logging_dir: &Path, entry: &CodexSessionLogEntry) -> PathBuf {
    logging_dir
        .join(SESSION_LOG_DIR)
        .join(session_log_file_name(entry))
}

fn session_log_file_name(entry: &CodexSessionLogEntry) -> String {
    let file_id = if entry.session_file_id.trim().is_empty() {
        entry.session_id.as_deref().unwrap_or("unknown")
    } else {
        &entry.session_file_id
    };
    format!(
        "{}-{}.jsonl",
        sanitize_workspace_key(&entry.issue_identifier),
        sanitize_workspace_key(file_id)
    )
}

fn session_log_paths(logging_dir: &Path, issue_identifier: &str) -> io::Result<Vec<PathBuf>> {
    let dir = logging_dir.join(SESSION_LOG_DIR);
    let read_dir = match fs::read_dir(&dir) {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let file_prefix = format!("{}-", sanitize_workspace_key(issue_identifier));
    let mut paths = Vec::new();
    for entry in read_dir {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if file_name.starts_with(&file_prefix) && file_name.ends_with(".jsonl") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn read_recent_session_logs(path: &Path, limit: usize) -> io::Result<Vec<CodexSessionLogEntry>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut position = file.seek(SeekFrom::End(0))?;
    let mut buffer = Vec::new();
    loop {
        if position == 0 {
            let mut entries = parse_session_log_buffer(&buffer, true);
            if entries.len() > limit {
                entries.drain(0..entries.len() - limit);
            }
            return Ok(entries);
        }
        let read_len = position.min(SESSION_LOG_TAIL_CHUNK_BYTES) as usize;
        position -= read_len as u64;
        file.seek(SeekFrom::Start(position))?;
        let mut chunk = vec![0; read_len];
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&buffer);
        buffer = chunk;

        let mut entries = parse_session_log_buffer(&buffer, position == 0);
        if entries.len() >= limit {
            entries.drain(0..entries.len() - limit);
            return Ok(entries);
        }
    }
}

fn last_sequence(path: &Path) -> io::Result<Option<u64>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut position = file.seek(SeekFrom::End(0))?;
    let mut buffer = Vec::new();
    loop {
        if position == 0 {
            return Ok(parse_session_log_buffer(&buffer, true)
                .into_iter()
                .rev()
                .find_map(|entry| (entry.sequence > 0).then_some(entry.sequence)));
        }
        let read_len = position.min(SESSION_LOG_TAIL_CHUNK_BYTES) as usize;
        position -= read_len as u64;
        file.seek(SeekFrom::Start(position))?;
        let mut chunk = vec![0; read_len];
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&buffer);
        buffer = chunk;

        if let Some(sequence) = parse_session_log_buffer(&buffer, position == 0)
            .into_iter()
            .rev()
            .find_map(|entry| (entry.sequence > 0).then_some(entry.sequence))
        {
            return Ok(Some(sequence));
        }
    }
}

fn parse_session_log_buffer(buffer: &[u8], includes_file_start: bool) -> Vec<CodexSessionLogEntry> {
    let start = if includes_file_start {
        0
    } else {
        match buffer.iter().position(|byte| *byte == b'\n') {
            Some(index) => index + 1,
            None => buffer.len(),
        }
    };
    String::from_utf8_lossy(&buffer[start..])
        .lines()
        .filter_map(parse_session_log_line)
        .collect()
}

fn parse_session_log_line(line: &str) -> Option<CodexSessionLogEntry> {
    if line.trim().is_empty() {
        return None;
    }
    match serde_json::from_str(line) {
        Ok(entry) => Some(entry),
        Err(err) => {
            tracing::warn!(error=%err, "session_log_line_parse outcome=skipped");
            None
        }
    }
}
