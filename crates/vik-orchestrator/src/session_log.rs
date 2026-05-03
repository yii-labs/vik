use std::collections::hash_map::RandomState;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::Value;
use tokio::task;
use vik_core::{
    AttemptSnapshot, CodexSessionLogEntry, IssueDebugSnapshot, RecentEvent, WorkspacePathSnapshot,
    sanitize_workspace_key,
};

const LEGACY_SESSION_LOG_DIR: &str = "codex-session-logs";
const SESSION_LOG_DIR: &str = "sessions";
const SESSION_LOG_EXT: &str = ".jsonl";
const SESSION_LOG_TAIL_CHUNK_BYTES: u64 = 16 * 1024;

pub(crate) fn new_session_log_id() -> String {
    format!("{}-{}", Utc::now().timestamp_millis(), random_hex_suffix())
}

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
    let (path, session_log_id) = append_session_log_path(logging_dir, entry)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut entry = entry.clone();
    entry.session_log_id = session_log_id;
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
    let paths = session_log_paths(logging_dir, issue_identifier)?;
    if limit > 0 {
        let mut chunks = Vec::new();
        let mut remaining = limit;
        for path in paths.iter().rev() {
            let entries = read_recent_session_logs(path, remaining)?;
            remaining = remaining.saturating_sub(entries.len());
            chunks.push(entries);
            if remaining == 0 {
                break;
            }
        }
        let mut entries = Vec::new();
        for mut chunk in chunks.into_iter().rev() {
            entries.append(&mut chunk);
        }
        if entries.len() > limit {
            entries.drain(0..entries.len() - limit);
        }
        return Ok(entries);
    }
    let mut entries = Vec::new();
    for path in paths {
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        for line in BufReader::new(file).lines() {
            let line = line?;
            if let Some(entry) = parse_session_log_line(&line) {
                entries.push(entry);
            }
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

fn append_session_log_path(
    logging_dir: &Path,
    entry: &CodexSessionLogEntry,
) -> io::Result<(PathBuf, String)> {
    if !entry.session_log_id.trim().is_empty() {
        let session_log_id = sanitize_workspace_key(&entry.session_log_id);
        return Ok((
            session_log_path(logging_dir, &entry.issue_identifier, &session_log_id),
            session_log_id,
        ));
    }
    if let Some(path) = latest_session_log_path(logging_dir, &entry.issue_identifier)? {
        if let Some(session_log_id) = session_log_id_from_path(&path, &entry.issue_identifier) {
            return Ok((path, session_log_id));
        }
    }
    let session_log_id = new_session_log_id();
    Ok((
        session_log_path(logging_dir, &entry.issue_identifier, &session_log_id),
        session_log_id,
    ))
}

fn session_log_path(logging_dir: &Path, issue_identifier: &str, session_log_id: &str) -> PathBuf {
    logging_dir.join(SESSION_LOG_DIR).join(format!(
        "{}-{}{}",
        sanitize_workspace_key(issue_identifier),
        sanitize_workspace_key(session_log_id),
        SESSION_LOG_EXT
    ))
}

fn latest_session_log_path(
    logging_dir: &Path,
    issue_identifier: &str,
) -> io::Result<Option<PathBuf>> {
    Ok(current_session_log_paths(logging_dir, issue_identifier)?.pop())
}

fn session_log_paths(logging_dir: &Path, issue_identifier: &str) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let legacy_path = logging_dir.join(LEGACY_SESSION_LOG_DIR).join(format!(
        "{}{}",
        sanitize_workspace_key(issue_identifier),
        SESSION_LOG_EXT
    ));
    if legacy_path.exists() {
        paths.push(legacy_path);
    }
    paths.extend(current_session_log_paths(logging_dir, issue_identifier)?);
    Ok(paths)
}

fn current_session_log_paths(
    logging_dir: &Path,
    issue_identifier: &str,
) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let dir = logging_dir.join(SESSION_LOG_DIR);
    match fs::read_dir(&dir) {
        Ok(entries) => {
            let prefix = format!("{}-", sanitize_workspace_key(issue_identifier));
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if file_name.starts_with(&prefix) && file_name.ends_with(SESSION_LOG_EXT) {
                    paths.push(path);
                }
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    paths.sort_by(|left, right| {
        left.file_name()
            .and_then(|name| name.to_str())
            .cmp(&right.file_name().and_then(|name| name.to_str()))
    });
    Ok(paths)
}

fn session_log_id_from_path(path: &Path, issue_identifier: &str) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    let prefix = format!("{}-", sanitize_workspace_key(issue_identifier));
    file_name
        .strip_prefix(&prefix)?
        .strip_suffix(SESSION_LOG_EXT)
        .filter(|session_log_id| !session_log_id.is_empty())
        .map(ToOwned::to_owned)
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

fn random_hex_suffix() -> String {
    let mut bytes = [0_u8; 8];
    if let Ok(mut file) = File::open("/dev/urandom")
        && file.read_exact(&mut bytes).is_ok()
    {
        return hex(&bytes);
    }

    let mut hasher = RandomState::new().build_hasher();
    if let Some(nanos) = Utc::now().timestamp_nanos_opt() {
        hasher.write_i64(nanos);
    }
    hasher.write_u32(std::process::id());
    hex(&hasher.finish().to_be_bytes())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}
