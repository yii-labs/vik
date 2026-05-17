//! Hook runner.
//!
//! Hooks are user-authored shell bodies fired at three workflow points:
//! `issue.hooks.after_create` (issue-level, restricted to `issue`+`env`
//! template context), and `hooks.before_run` / `hooks.after_run`
//! (stage-level, full stage context). All run via `sh -c` / `cmd /C`
//! through [`CommandExt`] with a 30s timeout, inheriting the daemon's
//! environment.
//!
//! Public surface is the trigger/join split: [`HookRunner::schedule_*`]
//! returns a [`HookTrigger`] (fire it to run the hook) plus a
//! [`HookJoin`] (await the outcome). This lets the orchestrator
//! schedule the hook at one point in the dispatch flow but fire it at
//! another — e.g. render eagerly, fire only after the stage workspace
//! is ready. Convenience wrappers (`run_*`) collapse the two halves
//! when the caller just wants fire-and-await.
//!
//! Hook failure short-circuits stage dispatch for that cycle; the next
//! intake cycle retries naturally because Vik writes no per-issue
//! marker.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;
use thiserror::Error;
use tokio::process::Command;
use tracing::Instrument;

use crate::context::{IssueRun, IssueStage};
use crate::logging::Phase;
use crate::shell::{CommandExecError, CommandExt};
use crate::template::{JinjaRenderer, TemplateError};

/// Mirrors the prompt-command timeout — both are user shell bodies
/// expected to complete quickly, and a single knob keeps mental
/// overhead low. Not workflow-configurable today.
const HOOK_TIMEOUT: Duration = Duration::from_secs(30);

/// Bounded so a runaway hook cannot flood the log stream.
const STDERR_TAIL_BYTES: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookKind {
  AfterIssueWorkdirCreate,
  BeforeIssueStageRun,
  AfterIssueStageRun,
}

impl HookKind {
  /// Matches the YAML key names so log output is grep-friendly against
  /// the workflow config.
  pub fn as_str(&self) -> &'static str {
    match self {
      HookKind::AfterIssueWorkdirCreate => "after_issue_workdir_create",
      HookKind::BeforeIssueStageRun => "before_issue_stage_run",
      HookKind::AfterIssueStageRun => "after_issue_stage_run",
    }
  }
}

#[derive(Debug, Error)]
pub enum HookError {
  #[error("hook `{hook}` template render failed: {source}")]
  Render {
    hook: &'static str,
    #[source]
    source: TemplateError,
  },

  #[error("hook `{hook}` shell execution failed: {source}")]
  Exec {
    hook: &'static str,
    #[source]
    source: CommandExecError,
  },

  /// `stderr_tail` is bounded by [`STDERR_TAIL_BYTES`].
  #[error("hook `{hook}` exited with non-zero status {code}: {stderr_tail}")]
  NonZeroExit {
    hook: &'static str,
    code: i32,
    stderr_tail: String,
  },
}

/// Cheap to clone (`JinjaRenderer` is `Clone`) so concurrent dispatch
/// tasks each get their own without lock contention.
#[derive(Debug, Clone)]
pub struct HookRunner {
  renderer: JinjaRenderer,
  timeout: Duration,
}

impl Default for HookRunner {
  fn default() -> Self {
    Self::new()
  }
}

impl HookRunner {
  pub fn new() -> Self {
    Self {
      renderer: JinjaRenderer::new(),
      timeout: HOOK_TIMEOUT,
    }
  }

  #[allow(dead_code)]
  pub fn with_timeout(mut self, timeout: Duration) -> Self {
    self.timeout = timeout;
    self
  }

  /// Template context is intentionally restricted to `issue` + `env`
  /// (no `stage`/`cwd`/`workspace`) because `after_create` runs at
  /// issue scope, before any stage has been chosen. The shell still
  /// runs in the issue workspace for convenience.
  ///
  /// Render errors surface at join time, not here, because the render
  /// happens on the spawned task — the caller is free to do
  /// concurrent setup before firing the trigger.
  #[inline]
  pub async fn after_issue_workdir_created(&self, issue: &IssueRun, hook: &Option<String>) -> Result<(), HookError> {
    self
      .schedule_inner(HookKind::AfterIssueWorkdirCreate, issue.workdir(), hook, issue)
      .await
  }

  #[inline]
  pub async fn before_issue_stage_run(&self, stage: &IssueStage, hook: &Option<String>) -> Result<(), HookError> {
    self
      .schedule_inner(HookKind::BeforeIssueStageRun, stage.workdir(), hook, stage)
      .await
  }

  #[inline]
  pub async fn after_issue_stage_run(&self, stage: &IssueStage, hook: &Option<String>) -> Result<(), HookError> {
    self
      .schedule_inner(HookKind::AfterIssueStageRun, stage.workdir(), hook, stage)
      .await
  }

  async fn schedule_inner<Context: Serialize>(
    &self,
    kind: HookKind,
    cwd: &Path,
    hook: &Option<String>,
    context: Context,
  ) -> Result<(), HookError> {
    let hook_name = kind.as_str();
    let _span = tracing::info_span!(
      "hook",
      phase = %Phase::Hook,
      hook = %hook_name,
    );

    let command = match hook {
      Some(body) => body,
      None => {
        tracing::debug!("hook not configured; skipping execution");
        return Ok(());
      },
    };

    let command = self.render_hook_command(kind, command, context)?;

    self.run_command(kind, cwd, command).in_current_span().await
  }

  fn render_hook_command<Context: Serialize>(
    &self,
    kind: HookKind,
    command: &str,
    context: Context,
  ) -> Result<String, HookError> {
    self.renderer.render(command, context).map_err(|e| HookError::Render {
      hook: kind.as_str(),
      source: e,
    })
  }

  async fn run_command(&self, kind: HookKind, cwd: &Path, command: String) -> Result<(), HookError> {
    let started = Instant::now();
    tracing::debug!(cwd = %cwd.display(), "hook shell starting");

    let output = match shell_command(&command).current_dir(cwd).timeout(self.timeout).output().await {
      Ok(output) => output,
      Err(source) => {
        let duration = started.elapsed().as_millis();
        tracing::error!(duration, error = %source, "hook shell exec errored");
        return Err(HookError::Exec {
          hook: kind.as_str(),
          source,
        });
      },
    };

    let duration = started.elapsed().as_millis();

    if output.status.success() {
      tracing::info!(duration, "hook completed");
      return Ok(());
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr_tail = tail_utf8(&output.stderr, STDERR_TAIL_BYTES);
    let error = HookError::NonZeroExit {
      hook: kind.as_str(),
      code,
      stderr_tail,
    };
    tracing::error!(duration, error = %error, "hook exited non-zero");
    Err(error)
  }
}

fn tail_utf8(bytes: &[u8], limit: usize) -> String {
  if bytes.len() <= limit {
    return String::from_utf8_lossy(bytes).into_owned();
  }
  let start = bytes.len() - limit;
  String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(windows)]
fn shell_command(body: &str) -> Command {
  let mut cmd = Command::new("cmd");
  cmd.args(["/C", body]);
  cmd
}

#[cfg(not(windows))]
fn shell_command(body: &str) -> Command {
  let mut cmd = Command::new("sh");
  cmd.args(["-c", body]);
  cmd
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hook_kind_names_match_workflow_keys() {
    assert_eq!(HookKind::AfterIssueWorkdirCreate.as_str(), "after_issue_workdir_create");
    assert_eq!(HookKind::BeforeIssueStageRun.as_str(), "before_issue_stage_run");
    assert_eq!(HookKind::AfterIssueStageRun.as_str(), "after_issue_stage_run");
  }

  #[tokio::test]
  async fn unconfigured_hook_skips_without_requiring_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing_cwd = temp.path().join("missing");

    HookRunner::new()
      .schedule_inner(
        HookKind::BeforeIssueStageRun,
        &missing_cwd,
        &None,
        serde_json::json!({}),
      )
      .await
      .expect("unconfigured hook skips");

    assert!(!missing_cwd.exists());
  }

  #[tokio::test]
  async fn configured_hook_renders_template_and_executes_in_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let hook = Some("echo {{ issue.id }}:{{ issue.stage.name }}>hook-output.txt".to_string());

    HookRunner::new()
      .schedule_inner(
        HookKind::BeforeIssueStageRun,
        temp.path(),
        &hook,
        serde_json::json!({
          "issue": {
            "id": "ISS-7",
            "stage": { "name": "plan" }
          },
        }),
      )
      .await
      .expect("configured hook runs");

    let output = std::fs::read_to_string(temp.path().join("hook-output.txt")).expect("hook output");
    assert_eq!(output.lines().next(), Some("ISS-7:plan"));
  }

  #[cfg(not(windows))]
  #[tokio::test]
  async fn nonzero_hook_reports_bounded_stderr_tail() {
    let temp = tempfile::tempdir().expect("tempdir");
    let stderr = format!("{}TAIL", "x".repeat(STDERR_TAIL_BYTES + 10));
    let hook = Some(format!("printf '%s' '{stderr}' >&2; exit 7"));

    let err = HookRunner::new()
      .schedule_inner(HookKind::AfterIssueStageRun, temp.path(), &hook, serde_json::json!({}))
      .await
      .expect_err("nonzero hook fails");

    match err {
      HookError::NonZeroExit {
        hook,
        code,
        stderr_tail,
      } => {
        assert_eq!(hook, "after_issue_stage_run");
        assert_eq!(code, 7);
        assert_eq!(stderr_tail, format!("{}TAIL", "x".repeat(STDERR_TAIL_BYTES - 4)));
      },
      other => panic!("expected nonzero exit, got {other:?}"),
    }
  }
}
