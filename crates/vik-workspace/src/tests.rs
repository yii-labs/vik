use std::collections::HashMap;
use std::path::Path;

use tempfile::tempdir;
use vik_workflow::HooksConfig;

use crate::{WorkspaceError, WorkspaceManager, ensure_inside_root};

fn hooks() -> HooksConfig {
    HooksConfig {
        timeout_ms: if cfg!(windows) { 10_000 } else { 2_000 },
        ..HooksConfig::default()
    }
}

fn append_marker_command(marker: &Path) -> String {
    let marker = marker.display().to_string();
    if cfg!(windows) {
        let marker = marker.replace('\'', "''");
        format!("'run' | Out-File -FilePath '{marker}' -Append -Encoding utf8")
    } else {
        let marker = marker.replace('\'', "'\\''");
        format!("printf 'run\\n' >> '{marker}'")
    }
}

fn append_env_marker_command(marker: &Path, label: &str, key: &str) -> String {
    let marker = marker.display().to_string();
    if cfg!(windows) {
        let marker = marker.replace('\'', "''");
        format!("'{label}=' + $env:{key} | Out-File -FilePath '{marker}' -Append -Encoding utf8")
    } else {
        let marker = marker.replace('\'', "'\\''");
        format!("printf '{label}=%s\\n' \"${key}\" >> '{marker}'")
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

#[tokio::test]
async fn hooks_receive_configured_environment() {
    let dir = tempdir().unwrap();
    let marker = dir.path().join("marker");
    let key = "VIK_WORKSPACE_HOOK_ENV";
    let mut config = hooks();
    config.after_create = Some(append_env_marker_command(&marker, "after_create", key));
    config.before_run = Some(append_env_marker_command(&marker, "before_run", key));
    config.after_run = Some(append_env_marker_command(&marker, "after_run", key));
    config.before_remove = Some(append_env_marker_command(&marker, "before_remove", key));
    let manager = WorkspaceManager::new(dir.path().join("root"), config)
        .with_env(HashMap::from([(key.to_string(), "hook-value".to_string())]));

    let workspace = manager.create_for_issue("ABC-1").await.unwrap();
    manager.before_run(&workspace.path).await.unwrap();
    manager.after_run_best_effort(&workspace.path).await;
    manager.remove_for_issue("ABC-1").await.unwrap();

    let marker_text = tokio::fs::read_to_string(marker).await.unwrap();
    let marker_text = marker_text.trim_start_matches('\u{feff}');
    assert_eq!(
        marker_text.lines().collect::<Vec<_>>(),
        [
            "after_create=hook-value",
            "before_run=hook-value",
            "after_run=hook-value",
            "before_remove=hook-value",
        ]
    );
}

#[test]
fn rejects_out_of_root_workspace_path() {
    let dir = tempdir().unwrap();
    let outside = dir.path().parent().unwrap().join("outside");
    let err = ensure_inside_root(dir.path(), &outside).unwrap_err();
    assert!(matches!(err, WorkspaceError::PathOutsideRoot));
}
