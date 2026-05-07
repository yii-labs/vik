//! `CommandExt` adds `.timeout(Duration)` to `tokio::process::Command`
//! and returns a [`Child`] that can be cancelled cooperatively.
//!
//! The timeout race and cancellation handling run in a spawned task so
//! the caller's `wait()` future races a `oneshot::Receiver` rather
//! than the child directly.

use std::io;
use std::process::{ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process;
use tokio::sync::oneshot;
use tokio::time;
use tokio_util::sync::CancellationToken;

use super::CommandExecError;

const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

pub trait CommandExt {
  fn timeout(&mut self, duration: Duration) -> Command<'_>;
}

impl CommandExt for process::Command {
  fn timeout(&mut self, duration: Duration) -> Command<'_> {
    let mut wrapped = Command::new(self);
    wrapped.timeout(duration);
    wrapped
  }
}

impl<'cmd> From<&'cmd mut process::Command> for Command<'cmd> {
  fn from(cmd: &'cmd mut process::Command) -> Self {
    Command::new(cmd)
  }
}

pub struct Command<'cmd> {
  inner: &'cmd mut process::Command,
  timeout: Duration,
}

impl Command<'_> {
  pub fn new(command: &'_ mut process::Command) -> Command<'_> {
    Command {
      inner: command,
      timeout: DEFAULT_COMMAND_TIMEOUT,
    }
  }

  pub fn timeout(&mut self, timeout: Duration) -> &mut Self {
    self.timeout = timeout;
    self
  }

  pub fn spawn(&mut self) -> Result<Child, CommandExecError> {
    match self.inner.spawn() {
      Ok(inner) => Ok(Child::new(self, inner)),
      Err(e) => Err(CommandExecError::Spawn(e)),
    }
  }

  pub fn output(&mut self) -> impl std::future::Future<Output = Result<Output, CommandExecError>> + '_ {
    self.inner.stdout(Stdio::piped());
    self.inner.stderr(Stdio::piped());

    let child = self.spawn();

    async { child?.wait_with_output().await }
  }

  #[allow(dead_code)]
  pub fn status(&mut self) -> impl std::future::Future<Output = Result<ExitStatus, CommandExecError>> + '_ {
    let child = self.spawn();

    async {
      let mut child = child?;

      child.stdin.take();
      child.stdout.take();
      child.stderr.take();

      child.wait().await
    }
  }
}

#[derive(Debug)]
pub struct Child {
  inner: ChildInner,
  pub stdin: Option<process::ChildStdin>,
  pub stdout: Option<process::ChildStdout>,
  pub stderr: Option<process::ChildStderr>,
}

impl Child {
  pub fn new(command: &Command, mut inner: process::Child) -> Self {
    let stdin = inner.stdin.take();
    let stdout = inner.stdout.take();
    let stderr = inner.stderr.take();

    Self {
      inner: ChildInner::Running(RunningChild::spin(command, inner)),
      stdin,
      stdout,
      stderr,
    }
  }

  pub fn cancel(&self) {
    match &self.inner {
      ChildInner::Running(running) => running.cancellation.cancel(),
      ChildInner::Done(_) | ChildInner::Failed => (),
    }
  }

  pub async fn wait(&mut self) -> Result<ExitStatus, CommandExecError> {
    // Drop stdin first so a child waiting on EOF can complete; without
    // this `wait` would deadlock against a child that reads stdin to
    // completion before exiting (Codex, Claude, common shells).
    drop(self.stdin.take());

    match &mut self.inner {
      ChildInner::Running(running) => match running.wait().await {
        Ok(status) => {
          self.inner = ChildInner::Done(status);
          Ok(status)
        },
        Err(e) => {
          self.inner = ChildInner::Failed;
          Err(e)
        },
      },
      ChildInner::Done(result) => Ok(*result),
      ChildInner::Failed => Err(CommandExecError::Spawn(io::Error::other(
        "subprocess wait result already failed",
      ))),
    }
  }

  pub async fn wait_with_output(&mut self) -> Result<Output, CommandExecError> {
    /// Logic borrowed from [`tokio::process::Child::wait_with_output`], but with added cancellation and timeout handling.
    async fn read_to_end<A: AsyncRead + Unpin>(io: &mut Option<A>) -> Result<Vec<u8>, CommandExecError> {
      let mut vec = Vec::new();
      if let Some(io) = io.as_mut() {
        io.read_to_end(&mut vec).await.map_err(CommandExecError::Spawn)?;
      }

      Ok(vec)
    }

    let mut stdout_pipe = self.stdout.take();
    let mut stderr_pipe = self.stderr.take();

    let stdout_fut = read_to_end(&mut stdout_pipe);
    let stderr_fut = read_to_end(&mut stderr_pipe);

    let (status, stdout, stderr) = futures::future::try_join3(self.wait(), stdout_fut, stderr_fut).await?;

    // Drop after the join because of <https://github.com/tokio-rs/tokio/issues/4309>.
    drop(stdout_pipe);
    drop(stderr_pipe);

    Ok(Output { status, stdout, stderr })
  }
}

#[derive(Debug)]
enum ChildInner {
  Running(RunningChild),
  Done(ExitStatus),
  Failed,
}

#[derive(Debug)]
struct RunningChild {
  cancellation: CancellationToken,
  result_rx: oneshot::Receiver<Result<ExitStatus, CommandExecError>>,
}

impl RunningChild {
  fn spin(spawner: &Command, inner: process::Child) -> Self {
    let started = Instant::now();
    let timeout = spawner.timeout;
    let cancellation = CancellationToken::new();
    let cancellation_signal = cancellation.clone();
    let (result_tx, result_rx) = oneshot::channel();

    // Background task races the child's exit against timeout and
    // cancellation. Lets `wait()` await a oneshot rather than the
    // child directly, which is what makes the timeout cooperative.
    tokio::spawn(async move {
      let result = wait_for_child(inner, started, timeout, cancellation_signal).await;
      let _ = result_tx.send(result);
    });

    Self {
      cancellation,
      result_rx,
    }
  }

  async fn wait(&mut self) -> Result<ExitStatus, CommandExecError> {
    match (&mut self.result_rx).await {
      Ok(result) => result,
      Err(_) => Err(CommandExecError::Spawn(io::Error::other(
        "subprocess dropped before reporting exit",
      ))),
    }
  }
}

async fn wait_for_child(
  mut child: process::Child,
  started: Instant,
  timeout: Duration,
  cancellation: CancellationToken,
) -> Result<ExitStatus, CommandExecError> {
  tokio::select! {
    res = child.wait() => {
      match res {
        Ok(status) => Ok(status),
        Err(err) => Err(CommandExecError::Spawn(err)),
      }
    },
    _ = time::sleep(timeout) => {
      // `child.kill()` is best-effort: if it fails the process tree
      // is already gone or in a zombie state we cannot recover here.
      if let Err(e) = child.kill().await {
        tracing::error!("Failed to kill process on timeout: {e}");
      }

      Err(CommandExecError::Timeout{
        duration_ms: timeout.as_millis() as u64
      })
    },
    _ = cancellation.cancelled() => {
      if let Err(e) = child.kill().await {
        tracing::error!("Failed to kill process on cancellation: {e}");
      }

      Err(CommandExecError::Cancelled {
        duration_ms: started.elapsed().as_millis() as u64
      })
    },
  }
}

#[cfg(all(test, target_family = "unix"))]
mod tests {
  use std::time::Duration;

  use tokio::io::AsyncWriteExt;

  use super::*;

  #[tokio::test]
  async fn echo_stdout_success() {
    let out = process::Command::new("echo")
      .arg("hello")
      .timeout(Duration::from_secs(1))
      .output()
      .await
      .unwrap();

    assert_eq!(out.stdout, b"hello\n");
  }

  #[tokio::test]
  async fn stdin_is_forwarded() {
    let mut child = process::Command::new("cat")
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .timeout(Duration::from_secs(1))
      .spawn()
      .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"hello").await.unwrap();
    drop(stdin);

    let out = child.wait_with_output().await.unwrap();

    assert_eq!(out.stdout, b"hello");
  }

  #[tokio::test]
  async fn nonzero_exit_is_captured() {
    let out = process::Command::new("sh")
      .args(["-c", "exit 3"])
      .timeout(Duration::from_secs(1))
      .spawn()
      .unwrap()
      .wait()
      .await
      .unwrap();

    assert!(!out.success());
    assert_eq!(out.code(), Some(3));
  }

  #[tokio::test]
  async fn stderr_is_captured() {
    let out = process::Command::new("sh")
      .args(["-c", "echo err 1>&2"])
      .timeout(Duration::from_secs(1))
      .output()
      .await
      .unwrap();

    assert_eq!(out.stderr, b"err\n");
  }

  #[tokio::test]
  async fn missing_program_is_spawn_error() {
    let err = process::Command::new("/definitely/not/a/real/binary")
      .timeout(Duration::from_secs(1))
      .spawn()
      .expect_err("spawn must fail");

    assert!(matches!(err, CommandExecError::Spawn(_)), "got {err:?}");
  }

  #[tokio::test]
  async fn sleep_beyond_timeout_returns_timeout() {
    tokio::time::pause();

    let mut child = process::Command::new("/bin/sh")
      .args(["-c", "sleep 5"])
      .timeout(Duration::from_millis(100))
      .spawn()
      .expect("sleep must spawn");

    tokio::time::advance(Duration::from_millis(200)).await;

    let err = child.wait().await.expect_err("sleep must time out");

    assert!(matches!(err, CommandExecError::Timeout { .. }), "got {err:?}");
  }

  #[tokio::test]
  async fn wait_after_exit_uses_background_status() {
    let mut child = process::Command::new("/bin/sh")
      .args(["-c", "exit 7"])
      .timeout(Duration::from_secs(1))
      .spawn()
      .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let status = child.wait().await.unwrap();

    assert_eq!(status.code(), Some(7));
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn run_stream_cancel_returns_cancelled() {
    let mut child = process::Command::new("/bin/sh")
      .args(["-c", "sleep 30"])
      .timeout(Duration::from_secs(1))
      .spawn()
      .unwrap();

    child.cancel();

    let err = child.wait().await.expect_err("cancel must error");

    assert!(matches!(err, CommandExecError::Cancelled { .. }), "got {err:?}");
  }
}
