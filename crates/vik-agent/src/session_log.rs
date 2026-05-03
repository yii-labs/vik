use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use vik_core::sanitize_workspace_key;

pub(crate) fn session_log_path(
    workspace_path: &Path,
    issue_id: &str,
    codex_session_id: &str,
) -> PathBuf {
    workspace_path.join(".vik").join("sessions").join(format!(
        "{}-{}.jsonl",
        sanitize_workspace_key(issue_id),
        sanitize_workspace_key(codex_session_id)
    ))
}

pub(crate) async fn ensure_session_log(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.flush().await
}

pub(crate) async fn append_session_message(path: &Path, message: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .await?;
    if file_has_torn_tail(&mut file).await? {
        file.write_all(b"\n").await?;
    }
    let mut line = serde_json::to_vec(message)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    line.push(b'\n');
    file.write_all(&line).await?;
    file.flush().await
}

async fn file_has_torn_tail(file: &mut fs::File) -> io::Result<bool> {
    let len = file.metadata().await?.len();
    if len == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(len - 1)).await?;
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte).await?;
    Ok(byte[0] != b'\n')
}

#[cfg(test)]
pub(crate) async fn read_session_messages(path: &Path) -> io::Result<Vec<Value>> {
    let content = fs::read_to_string(path).await?;
    Ok(content
        .lines()
        .filter_map(|line| {
            if line.trim().is_empty() {
                None
            } else {
                serde_json::from_str(line).ok()
            }
        })
        .collect())
}
