use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use vik_core::sanitize_workspace_key;

pub(crate) fn session_log_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".vik").join("sessions")
}

pub(crate) fn session_log_path(
    session_log_dir: &Path,
    issue_identifier: &str,
    session_id: &str,
) -> PathBuf {
    let issue_identifier = sanitize_workspace_key(issue_identifier);
    let session_id = sanitize_workspace_key(session_id);
    session_log_dir.join(format!("{issue_identifier}-{session_id}.jsonl"))
}

pub(crate) struct SessionLog {
    path: PathBuf,
    file: File,
}

impl SessionLog {
    pub(crate) async fn open(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .await?;
        ensure_newline_boundary(&mut file).await?;
        Ok(Self { path, file })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) async fn append_message(&mut self, message: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(message).map_err(io::Error::other)?;
        line.push(b'\n');
        self.file.write_all(&line).await?;
        self.file.flush().await
    }
}

async fn ensure_newline_boundary(file: &mut tokio::fs::File) -> io::Result<()> {
    let len = file.metadata().await?.len();
    if len == 0 {
        return Ok(());
    }

    file.seek(std::io::SeekFrom::End(-1)).await?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last).await?;
    if last[0] != b'\n' {
        file.write_all(b"\n").await?;
    }
    Ok(())
}
