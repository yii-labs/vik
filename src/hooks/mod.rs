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

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::config::{IssueHooks, IssueStageHooks};
use crate::context::Issue;
use crate::logging::Phase;
use crate::shell::{CommandExecError, CommandExt};
use crate::template::{Context as TemplateContext, JinjaRenderer, StageContext, TemplateError, issue_value};

/// Mirrors the prompt-command timeout — both are user shell bodies
/// expected to complete quickly, and a single knob keeps mental
/// overhead low. Not workflow-configurable today.
const HOOK_TIMEOUT: Duration = Duration::from_secs(30);

/// Bounded so a runaway hook cannot flood the log stream.
const STDERR_TAIL_BYTES: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookKind {
  AfterWorkspaceCreate,
  BeforeStageRun,
  AfterStageRun,
}

impl HookKind {
  /// Matches the YAML key names so log output is grep-friendly against
  /// the workflow config.
  pub fn as_str(&self) -> &'static str {
    match self {
      HookKind::AfterWorkspaceCreate => "after_create",
      HookKind::BeforeStageRun => "before_run",
      HookKind::AfterStageRun => "after_run",
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

  /// Soft cancel — the trigger was dropped without firing. Distinct
  /// from a real failure so callers can decide whether to treat
  /// "hook never ran" as fatal.
  #[error("hook `{hook}` trigger dropped before firing")]
  Cancelled { hook: &'static str },
}

impl HookError {
  fn hook_name(&self) -> &'static str {
    match self {
      HookError::Render { hook, .. }
      | HookError::Exec { hook, .. }
      | HookError::NonZeroExit { hook, .. }
      | HookError::Cancelled { hook } => hook,
    }
  }
}

#[derive(Debug)]
pub enum HookOutcome {
  /// `None` body in the workflow. The runner still produces a trigger
  /// so callers do not branch on hook presence — firing just resolves
  /// here immediately.
  NotConfigured {
    kind: HookKind,
  },
  Ok {
    kind: HookKind,
    duration: Duration,
  },
  Failed {
    kind: HookKind,
    error: HookError,
  },
}

impl HookOutcome {
  /// Collapses outcomes to a Result for callers that want
  /// "continue-or-abort" semantics — both `Ok` and `NotConfigured`
  /// allow the dispatch cycle to proceed.
  pub fn into_result(self) -> Result<HookKind, HookError> {
    match self {
      HookOutcome::NotConfigured { kind } | HookOutcome::Ok { kind, .. } => Ok(kind),
      HookOutcome::Failed { error, .. } => Err(error),
    }
  }

  pub fn kind(&self) -> HookKind {
    match self {
      HookOutcome::NotConfigured { kind } | HookOutcome::Ok { kind, .. } | HookOutcome::Failed { kind, .. } => *kind,
    }
  }
}

/// Drop without firing → the paired [`HookJoin`] resolves with
/// `Failed(Cancelled)`. The `must_use` lint catches the easy mistake
/// of forgetting to call `fire`.
#[must_use = "dropping a HookTrigger without firing cancels the hook; call `fire` to run it"]
pub struct HookTrigger {
  kind: HookKind,
  sender: oneshot::Sender<()>,
}

impl HookTrigger {
  pub fn kind(&self) -> HookKind {
    self.kind
  }

  pub fn fire(self) {
    // `send` returns `Err` only if the receiver was already dropped —
    // task panic/abort. The caller still observes terminal state
    // through the paired `HookJoin`.
    let _ = self.sender.send(());
  }
}

/// Resolves to a [`HookOutcome`] even on task panic — a synthetic
/// `Failed(Cancelled)` is produced so callers never have to handle a
/// `JoinError` separately. The underlying `JoinError` is logged at
/// ERROR level so CI does not swallow it silently.
#[must_use = "the hook outcome must be awaited to learn whether dispatch can continue"]
pub struct HookJoin {
  kind: HookKind,
  handle: JoinHandle<HookOutcome>,
}

impl HookJoin {
  pub fn kind(&self) -> HookKind {
    self.kind
  }

  /// Await the hook's terminal outcome.
  pub async fn join(self) -> HookOutcome {
    match self.handle.await {
      Ok(outcome) => outcome,
      Err(err) => {
        tracing::error!(
            phase = %Phase::Hook,
            hook = %self.kind.as_str(),
            error = %err,
            "hook task panicked or was aborted",
        );
        HookOutcome::Failed {
          kind: self.kind,
          error: HookError::Cancelled {
            hook: self.kind.as_str(),
          },
        }
      },
    }
  }
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

  #[cfg(test)]
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
  pub fn schedule_after_create(
    &self,
    hooks: &IssueHooks,
    issue: &Issue,
    issue_workdir: &Path,
  ) -> (HookTrigger, HookJoin) {
    let kind = HookKind::AfterWorkspaceCreate;

    // Span carries the identifying fields once; the spawned task
    // inherits via `.instrument(span)` so inner log sites do not have
    // to thread `issue_id` etc. through the call chain.
    let span = tracing::info_span!(
      "hook",
      phase = %Phase::Hook,
      hook = kind.as_str(),
      issue_id = %issue.id,
      stage_name = "",
    );

    let invocation = HookInvocation::AfterWorkspaceCreate {
      body: hooks.after_create.clone(),
      context: after_create_context(issue),
      cwd: issue_workdir.to_path_buf(),
    };

    span.in_scope(|| self.schedule_inner(kind, invocation, span.clone()))
  }

  pub fn schedule_before_run(&self, hooks: &IssueStageHooks, ctx: StageContext<'_>) -> (HookTrigger, HookJoin) {
    let kind = HookKind::BeforeStageRun;

    let span = tracing::info_span!(
      "hook",
      phase = %Phase::Hook,
      hook = kind.as_str(),
      issue_id = %ctx.issue.id,
      stage_name = %ctx.stage_name,
    );

    let (context, cwd) = ctx.build_template_context_with_cwd();
    let invocation = HookInvocation::BeforeStageRun {
      body: hooks.before_run.clone(),
      context,
      cwd,
    };

    span.in_scope(|| self.schedule_inner(kind, invocation, span.clone()))
  }

  pub fn schedule_after_run(&self, hooks: &IssueStageHooks, ctx: StageContext<'_>) -> (HookTrigger, HookJoin) {
    let kind = HookKind::AfterStageRun;

    let span = tracing::info_span!(
      "hook",
      phase = %Phase::Hook,
      hook = kind.as_str(),
      issue_id = %ctx.issue.id,
      stage_name = %ctx.stage_name,
    );

    let (context, cwd) = ctx.build_template_context_with_cwd();
    let invocation = HookInvocation::AfterStageRun {
      body: hooks.after_run.clone(),
      context,
      cwd,
    };

    span.in_scope(|| self.schedule_inner(kind, invocation, span.clone()))
  }

  /// Fire-and-await wrapper. `Ok(())` covers both "ran fine" and "not
  /// configured"; callers wanting the deferred-fire pattern stay on
  /// `schedule_after_create` directly.
  pub async fn run_after_create(
    &self,
    hooks: &IssueHooks,
    issue: &Issue,
    issue_workdir: &Path,
  ) -> Result<(), HookError> {
    let (trigger, join) = self.schedule_after_create(hooks, issue, issue_workdir);
    trigger.fire();
    join.join().await.into_result().map(|_| ())
  }

  pub async fn run_before_run(&self, hooks: &IssueStageHooks, ctx: StageContext<'_>) -> Result<(), HookError> {
    let (trigger, join) = self.schedule_before_run(hooks, ctx);
    trigger.fire();
    join.join().await.into_result().map(|_| ())
  }

  pub async fn run_after_run(&self, hooks: &IssueStageHooks, ctx: StageContext<'_>) -> Result<(), HookError> {
    let (trigger, join) = self.schedule_after_run(hooks, ctx);
    trigger.fire();
    join.join().await.into_result().map(|_| ())
  }

  fn schedule_inner(&self, kind: HookKind, invocation: HookInvocation, span: tracing::Span) -> (HookTrigger, HookJoin) {
    let (tx, rx) = oneshot::channel::<()>();
    let renderer = self.renderer.clone();
    let timeout = self.timeout;

    let handle = tokio::spawn(async move { execute(kind, invocation, renderer, rx, timeout).await }.instrument(span));

    (HookTrigger { kind, sender: tx }, HookJoin { kind, handle })
  }
}

/// One variant per hook kind so adding a new kind means adding a
/// variant with its own shape, not growing optional fields on a
/// shared struct. `Debug` is intentionally not derived because
/// `TemplateContext` does not implement `Debug`.
enum HookInvocation {
  AfterWorkspaceCreate {
    body: Option<String>,
    context: TemplateContext,
    cwd: PathBuf,
  },
  BeforeStageRun {
    body: Option<String>,
    context: TemplateContext,
    cwd: PathBuf,
  },
  AfterStageRun {
    body: Option<String>,
    context: TemplateContext,
    cwd: PathBuf,
  },
}

impl HookInvocation {
  /// Internal helper so the runner loop stays variant-agnostic while
  /// the public shape forbids `Option`-typed "field absent" markers.
  fn into_parts(self) -> (Option<String>, TemplateContext, PathBuf) {
    match self {
      HookInvocation::AfterWorkspaceCreate { body, context, cwd }
      | HookInvocation::BeforeStageRun { body, context, cwd }
      | HookInvocation::AfterStageRun { body, context, cwd } => (body, context, cwd),
    }
  }
}

fn after_create_context(issue: &Issue) -> TemplateContext {
  let mut context = TemplateContext::new();
  // Empty path suppresses `issue.workdir` in the rendered context.
  // `after_create` is contractually issue-scoped, so exposing a
  // workdir here would let operators reach for stage-scoped paths
  // before any stage has been picked.
  context.with("issue", issue_value(issue, Path::new("")));
  context
}

async fn execute(
  kind: HookKind,
  invocation: HookInvocation,
  renderer: JinjaRenderer,
  rx: oneshot::Receiver<()>,
  timeout: Duration,
) -> HookOutcome {
  let hook_name = kind.as_str();
  let (body, context, cwd) = invocation.into_parts();

  // Render eagerly so a bad template fails fast — but hold the
  // outcome until the trigger fires. Callers expect "the hook ran
  // because I fired it" semantics, even when the failure was render
  // time.
  let rendered = match body {
    Some(ref raw) => match renderer.render(raw, &context.build()) {
      Ok(r) => Some(r),
      Err(source) => {
        return wait_for_fire_then_fail(
          rx,
          kind,
          HookError::Render {
            hook: hook_name,
            source,
          },
        )
        .await;
      },
    },
    None => None,
  };

  if rx.await.is_err() {
    // Trigger was dropped without firing.
    return HookOutcome::Failed {
      kind,
      error: HookError::Cancelled { hook: hook_name },
    };
  }

  let Some(rendered) = rendered else {
    tracing::debug!("hook not configured; skipping execution");
    return HookOutcome::NotConfigured { kind };
  };

  run_shell(kind, &rendered, &cwd, timeout).await
}

async fn wait_for_fire_then_fail(rx: oneshot::Receiver<()>, kind: HookKind, error: HookError) -> HookOutcome {
  // Wait for the trigger so the failure surfaces at the same lifecycle
  // point as a success would. If the caller never fires, the sender
  // drops and `await` returns `Err`, which still resolves us.
  let _ = rx.await;
  tracing::error!(error = %error, "hook failed");
  HookOutcome::Failed { kind, error }
}

async fn run_shell(kind: HookKind, rendered: &str, cwd: &Path, timeout: Duration) -> HookOutcome {
  let hook_name = kind.as_str();
  let started = Instant::now();

  let mut cmd = shell_command(rendered);
  cmd.current_dir(cwd);

  tracing::debug!(cwd = %cwd.display(), "hook shell starting");

  let output = match cmd.timeout(timeout).output().await {
    Ok(output) => output,
    Err(source) => {
      let duration_ms = started.elapsed().as_millis() as u64;
      let error = HookError::Exec {
        hook: hook_name,
        source,
      };
      tracing::error!(duration_ms, error = %error, "hook shell exec errored");
      return HookOutcome::Failed { kind, error };
    },
  };

  let duration = started.elapsed();
  let duration_ms = duration.as_millis() as u64;

  if output.status.success() {
    tracing::info!(duration_ms, "hook completed");
    return HookOutcome::Ok { kind, duration };
  }

  let code = output.status.code().unwrap_or(-1);
  let stderr_tail = tail_utf8(&output.stderr, STDERR_TAIL_BYTES);
  let error = HookError::NonZeroExit {
    hook: hook_name,
    code,
    stderr_tail,
  };
  tracing::error!(duration_ms, error = %error, "hook exited non-zero");
  HookOutcome::Failed { kind, error }
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

/// `HookError::hook_name` exists for callers that want to tag a parent
/// span with the failing hook's name without matching every variant.
/// Only test code exercises it today.
#[allow(dead_code)]
fn assert_hook_error_accessor(e: &HookError) -> &'static str {
  e.hook_name()
}

#[cfg(all(test, target_family = "unix"))]
mod tests {
  use super::*;
  use std::time::Duration;

  fn issue(id: &str) -> Issue {
    Issue {
      id: id.to_string(),
      title: "t".to_string(),
      description: "d".to_string(),
      state: "todo".to_string(),
      extra_payload: serde_yaml::Mapping::new(),
    }
  }

  #[tokio::test]
  async fn unconfigured_hook_resolves_with_not_configured() {
    let runner = HookRunner::new();
    let hooks = IssueHooks::default();
    let iss = issue("VIK-1");
    let tmp = tempfile::tempdir().unwrap();

    let (trigger, join) = runner.schedule_after_create(&hooks, &iss, tmp.path());
    trigger.fire();

    let outcome = join.join().await;
    assert!(matches!(
      outcome,
      HookOutcome::NotConfigured {
        kind: HookKind::AfterWorkspaceCreate
      }
    ));
  }

  #[tokio::test]
  async fn after_create_happy_path_runs_body() {
    let runner = HookRunner::new();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("after_create.marker");

    let mut hooks = IssueHooks::default();
    hooks.after_create = Some(format!("printf '{{{{ issue.id }}}}' > {}", marker.display()));
    let iss = issue("VIK-HAPPY");

    let (trigger, join) = runner.schedule_after_create(&hooks, &iss, tmp.path());
    trigger.fire();

    let outcome = join.join().await;
    assert!(matches!(outcome, HookOutcome::Ok { .. }), "got {outcome:?}");

    let written = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(written, "VIK-HAPPY", "template must render issue.id");
  }

  #[tokio::test]
  async fn before_run_renders_stage_context() {
    let runner = HookRunner::new();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("stage.txt");

    let mut hooks = IssueStageHooks::default();
    hooks.before_run = Some(format!(
      "printf '{{{{ stage.name }}}}:{{{{ stage.agent }}}}:{{{{ issue.id }}}}' > {}",
      marker.display()
    ));
    let iss = issue("VIK-CTX");
    let ctx = StageContext {
      issue: &iss,
      stage_name: "plan",
      agent_profile: "codex",
      stage_state: "todo",
      issue_workdir: tmp.path(),
      workspace_root: tmp.path(),
    };

    let (trigger, join) = runner.schedule_before_run(&hooks, ctx);
    trigger.fire();

    let outcome = join.join().await;
    assert!(matches!(outcome, HookOutcome::Ok { .. }), "got {outcome:?}");

    let written = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(written, "plan:codex:VIK-CTX");
  }

  #[tokio::test]
  async fn oneshot_trigger_delays_execution_until_fired() {
    let runner = HookRunner::new();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("fire.marker");

    let mut hooks = IssueHooks::default();
    hooks.after_create = Some(format!("touch {}", marker.display()));
    let iss = issue("VIK-FIRE");

    let (trigger, join) = runner.schedule_after_create(&hooks, &iss, tmp.path());

    // Let the spawned task park on the oneshot receiver.
    for _ in 0..10 {
      tokio::task::yield_now().await;
    }
    assert!(!marker.exists(), "hook must not run before trigger.fire()");

    trigger.fire();

    let outcome = join.join().await;
    assert!(matches!(outcome, HookOutcome::Ok { .. }));
    assert!(marker.exists(), "hook must run after trigger.fire()");
  }

  #[tokio::test]
  async fn trigger_dropped_without_fire_produces_cancelled() {
    let runner = HookRunner::new();
    let mut hooks = IssueHooks::default();
    hooks.after_create = Some("echo nope".to_string());
    let iss = issue("VIK-DROP");
    let tmp = tempfile::tempdir().unwrap();

    let (trigger, join) = runner.schedule_after_create(&hooks, &iss, tmp.path());
    drop(trigger);

    let outcome = join.join().await;
    assert!(
      matches!(
        outcome,
        HookOutcome::Failed {
          error: HookError::Cancelled { .. },
          ..
        }
      ),
      "got {outcome:?}"
    );
  }

  #[tokio::test]
  async fn render_failure_propagates_as_failed() {
    let runner = HookRunner::new();
    let mut hooks = IssueHooks::default();
    // `unknown_var` is not in the restricted `after_create` context.
    hooks.after_create = Some("echo {{ unknown_var }}".to_string());
    let iss = issue("VIK-BAD");
    let tmp = tempfile::tempdir().unwrap();

    let (trigger, join) = runner.schedule_after_create(&hooks, &iss, tmp.path());
    trigger.fire();

    let outcome = join.join().await;
    match outcome {
      HookOutcome::Failed {
        error: HookError::Render { hook, .. },
        ..
      } => {
        assert_eq!(hook, "after_create");
      },
      other => panic!("expected Render failure, got {other:?}"),
    }
  }

  #[tokio::test]
  async fn nonzero_exit_propagates_as_failed_with_stderr_tail() {
    let runner = HookRunner::new();
    let mut hooks = IssueStageHooks::default();
    hooks.before_run = Some("echo boom 1>&2; exit 7".to_string());
    let iss = issue("VIK-EXIT");
    let tmp = tempfile::tempdir().unwrap();
    let ctx = StageContext {
      issue: &iss,
      stage_name: "plan",
      agent_profile: "codex",
      stage_state: "todo",
      issue_workdir: tmp.path(),
      workspace_root: tmp.path(),
    };

    let (trigger, join) = runner.schedule_before_run(&hooks, ctx);
    trigger.fire();

    let outcome = join.join().await;
    match outcome {
      HookOutcome::Failed {
        error: HookError::NonZeroExit {
          code,
          stderr_tail,
          hook,
        },
        ..
      } => {
        assert_eq!(hook, "before_run");
        assert_eq!(code, 7);
        assert!(stderr_tail.contains("boom"), "stderr_tail was {stderr_tail:?}");
      },
      other => panic!("expected NonZeroExit, got {other:?}"),
    }
  }

  #[tokio::test]
  async fn shell_timeout_propagates_as_exec_failure() {
    let runner = HookRunner::new().with_timeout(Duration::from_millis(50));
    let mut hooks = IssueStageHooks::default();
    hooks.after_run = Some("sleep 5".to_string());
    let iss = issue("VIK-SLEEP");
    let tmp = tempfile::tempdir().unwrap();
    let ctx = StageContext {
      issue: &iss,
      stage_name: "plan",
      agent_profile: "codex",
      stage_state: "todo",
      issue_workdir: tmp.path(),
      workspace_root: tmp.path(),
    };

    let (trigger, join) = runner.schedule_after_run(&hooks, ctx);
    trigger.fire();

    let outcome = join.join().await;
    match outcome {
      HookOutcome::Failed {
        error: HookError::Exec {
          source: CommandExecError::Timeout { .. },
          hook,
        },
        ..
      } => {
        assert_eq!(hook, "after_run");
      },
      other => panic!("expected Exec(Timeout), got {other:?}"),
    }
  }

  #[tokio::test]
  async fn into_result_maps_outcome_to_result() {
    let ok = HookOutcome::Ok {
      kind: HookKind::BeforeStageRun,
      duration: Duration::from_millis(1),
    };
    assert!(matches!(ok.into_result(), Ok(HookKind::BeforeStageRun)));

    let skipped = HookOutcome::NotConfigured {
      kind: HookKind::AfterWorkspaceCreate,
    };
    assert!(matches!(skipped.into_result(), Ok(HookKind::AfterWorkspaceCreate)));

    let failed = HookOutcome::Failed {
      kind: HookKind::AfterStageRun,
      error: HookError::Cancelled { hook: "after_run" },
    };
    let err = failed.into_result().expect_err("must be err");
    assert_eq!(err.hook_name(), "after_run");
  }

  #[tokio::test]
  async fn run_after_create_collapses_trigger_and_join() {
    let runner = HookRunner::new();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("run_after_create.marker");

    let mut hooks = IssueHooks::default();
    hooks.after_create = Some(format!("printf ran > {}", marker.display()));
    let iss = issue("VIK-WRAP-OK");

    runner
      .run_after_create(&hooks, &iss, tmp.path())
      .await
      .expect("wrapper must succeed");

    assert!(marker.exists(), "wrapper must actually fire the hook");
  }

  #[tokio::test]
  async fn run_after_create_skips_unconfigured_as_ok() {
    let runner = HookRunner::new();
    let hooks = IssueHooks::default();
    let iss = issue("VIK-WRAP-NONE");
    let tmp = tempfile::tempdir().unwrap();

    runner
      .run_after_create(&hooks, &iss, tmp.path())
      .await
      .expect("no body => Ok(()), not Err");
  }

  #[tokio::test]
  async fn run_before_run_propagates_failure() {
    let runner = HookRunner::new();
    let mut hooks = IssueStageHooks::default();
    hooks.before_run = Some("exit 3".to_string());
    let iss = issue("VIK-WRAP-FAIL");
    let tmp = tempfile::tempdir().unwrap();
    let ctx = StageContext {
      issue: &iss,
      stage_name: "plan",
      agent_profile: "codex",
      stage_state: "todo",
      issue_workdir: tmp.path(),
      workspace_root: tmp.path(),
    };

    let err = runner
      .run_before_run(&hooks, ctx)
      .await
      .expect_err("non-zero exit must surface as Err");
    match err {
      HookError::NonZeroExit { code, hook, .. } => {
        assert_eq!(hook, "before_run");
        assert_eq!(code, 3);
      },
      other => panic!("expected NonZeroExit, got {other:?}"),
    }
  }

  #[tokio::test]
  async fn run_after_run_propagates_render_failure() {
    let runner = HookRunner::new();
    let mut hooks = IssueStageHooks::default();
    hooks.after_run = Some("echo {{ missing_var }}".to_string());
    let iss = issue("VIK-WRAP-RENDER");
    let tmp = tempfile::tempdir().unwrap();
    let ctx = StageContext {
      issue: &iss,
      stage_name: "plan",
      agent_profile: "codex",
      stage_state: "todo",
      issue_workdir: tmp.path(),
      workspace_root: tmp.path(),
    };

    let err = runner
      .run_after_run(&hooks, ctx)
      .await
      .expect_err("render failure must bubble up");
    assert!(
      matches!(err, HookError::Render { hook: "after_run", .. }),
      "got {err:?}"
    );
  }

  #[tokio::test]
  async fn env_is_reachable_from_template() {
    // SAFETY: tests run in-process; set a unique key name so no other
    // test races on the same env slot.
    //
    // `set_var` is unsafe in Rust 2024; gate on the narrow use here.
    // A collision-resistant key name plus the single-set-no-unset pattern
    // keeps this safe in practice — `std::env::set_var` races on
    // multi-threaded test runners in theory.
    unsafe { std::env::set_var("VIK_HOOK_TEST_42", "payload") };
    let runner = HookRunner::new();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("env.marker");

    let mut hooks = IssueHooks::default();
    hooks.after_create = Some(format!(
      "printf '{{{{ env.VIK_HOOK_TEST_42 }}}}' > {}",
      marker.display()
    ));
    let iss = issue("VIK-ENV");

    let (trigger, join) = runner.schedule_after_create(&hooks, &iss, tmp.path());
    trigger.fire();

    let outcome = join.join().await;
    assert!(matches!(outcome, HookOutcome::Ok { .. }), "got {outcome:?}");

    let written = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(written, "payload");
  }
}
