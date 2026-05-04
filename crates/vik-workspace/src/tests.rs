use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::tempdir;
use vik_workflow::{HooksConfig, RepoCloneConfig, RepoConfig};

use crate::{WorkspaceError, WorkspaceManager, ensure_inside_root};

fn hooks() -> HooksConfig {
    HooksConfig {
        timeout_ms: 10_000,
        ..HooksConfig::default()
    }
}

fn append_marker_command(marker: &Path) -> String {
    let marker = marker.display().to_string();
    if cfg!(windows) {
        let marker = marker.replace('\'', "''");
        format!(
            "[System.IO.File]::AppendAllText('{marker}', \"run`n\", [System.Text.UTF8Encoding]::new($false))"
        )
    } else {
        let marker = marker.replace('\'', "'\\''");
        format!("printf 'run\\n' >> '{marker}'")
    }
}

fn git(args: &[&str], cwd: &Path) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn create_bare_repo(root: &Path) -> String {
    let source = root.join("source");
    let bare = root.join("origin.git");
    fs::create_dir_all(&source).unwrap();
    git(&["init", "."], &source);
    git(&["config", "user.email", "vik@example.com"], &source);
    git(&["config", "user.name", "Vik Test"], &source);
    fs::write(source.join("tracked.txt"), "first\n").unwrap();
    git(&["add", "tracked.txt"], &source);
    git(&["commit", "-m", "first"], &source);
    fs::write(source.join("tracked.txt"), "second\n").unwrap();
    git(&["add", "tracked.txt"], &source);
    git(&["commit", "-m", "second"], &source);
    let bare_arg = bare.to_string_lossy().to_string();
    git(&["clone", "--bare", ".", &bare_arg], &source);
    file_url(&bare)
}

fn file_url(path: &Path) -> String {
    let path = path.display().to_string().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

fn repo_clone_marker_command(marker: &Path) -> String {
    let marker = marker.display().to_string();
    if cfg!(windows) {
        let marker = marker.replace('\'', "''");
        format!(
            "$inside = git rev-parse --is-inside-work-tree; [System.IO.File]::WriteAllText('{marker}', $inside, [System.Text.UTF8Encoding]::new($false))"
        )
    } else {
        let marker = marker.replace('\'', "'\\''");
        format!("git rev-parse --is-inside-work-tree > '{marker}'")
    }
}

#[tokio::test]
async fn creates_sanitized_workspace_once() {
    let dir = tempdir().unwrap();
    let manager = WorkspaceManager::new(dir.path(), hooks());
    let first = manager.create_for_issue("ABC/1 bad").await.unwrap();
    let second = manager.create_for_issue("ABC/1 bad").await.unwrap();
    assert_eq!(first.workspace_key, "ABC_1_bad");
    assert!(first.created_now);
    assert!(!second.created_now);
    assert_eq!(first.path, second.path);
}

#[tokio::test]
async fn after_create_runs_only_for_new_workspace() {
    let dir = tempdir().unwrap();
    let marker = dir.path().join("marker");
    let mut config = hooks();
    config.after_create = Some(append_marker_command(&marker));
    let manager = WorkspaceManager::new(dir.path().join("root"), config);
    manager.create_for_issue("ABC-1").await.unwrap();
    manager.create_for_issue("ABC-1").await.unwrap();
    let marker_text = tokio::fs::read_to_string(marker).await.unwrap();
    assert_eq!(marker_text.lines().count(), 1);
}

#[tokio::test]
async fn repo_clone_runs_before_after_create_and_applies_depth() {
    let dir = tempdir().unwrap();
    let origin = create_bare_repo(dir.path());
    let marker = dir.path().join("marker");
    let mut config = hooks();
    config.after_create = Some(repo_clone_marker_command(&marker));
    let repo = RepoConfig {
        origin,
        clone: RepoCloneConfig { depth: Some(1) },
    };
    let manager = WorkspaceManager::with_repo(dir.path().join("root"), config, Some(repo));

    let workspace = manager.create_for_issue("ABC-1").await.unwrap();

    let marker_text = tokio::fs::read_to_string(marker).await.unwrap();
    assert_eq!(marker_text.trim(), "true");
    let tracked_text = tokio::fs::read_to_string(workspace.path.join("tracked.txt"))
        .await
        .unwrap();
    assert_eq!(tracked_text.lines().next(), Some("second"));
    assert_eq!(git(&["rev-list", "--count", "HEAD"], &workspace.path), "1");
}

#[tokio::test]
async fn repo_clone_failure_removes_new_workspace() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("missing.git").to_string_lossy().to_string();
    let repo = RepoConfig {
        origin: missing,
        clone: RepoCloneConfig { depth: Some(1) },
    };
    let manager = WorkspaceManager::with_repo(dir.path().join("root"), hooks(), Some(repo));

    let err = manager.create_for_issue("ABC-1").await.unwrap_err();

    assert!(matches!(err, WorkspaceError::RepoCloneFailed { .. }));
    assert!(
        tokio::fs::metadata(dir.path().join("root").join("ABC-1"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn before_run_failure_aborts() {
    let dir = tempdir().unwrap();
    let mut config = hooks();
    config.before_run = Some("exit 7".to_string());
    let manager = WorkspaceManager::new(dir.path(), config);
    let workspace = manager.create_for_issue("ABC-1").await.unwrap();
    let err = manager.before_run(&workspace.path).await.unwrap_err();
    assert!(matches!(
        err,
        WorkspaceError::HookFailed {
            hook: "before_run",
            status: 7
        }
    ));
}

#[test]
fn rejects_out_of_root_workspace_path() {
    let dir = tempdir().unwrap();
    let outside = dir.path().parent().unwrap().join("outside");
    let err = ensure_inside_root(dir.path(), &outside).unwrap_err();
    assert!(matches!(err, WorkspaceError::PathOutsideRoot));
}
