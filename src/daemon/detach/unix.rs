//! Unix double-fork detach.
//!
//! Sequence:
//!
//! 1. `pipe()` for the parent/child handshake.
//! 2. First `fork`. Parent waits on the read end for a one-byte status
//!    code, then `_exit(0)`s. Child closes the read end and continues.
//! 3. `setsid()` — the child becomes a new session leader, detaching
//!    from the controlling terminal.
//! 4. Second `fork`. Intermediate process `_exit`s; the grandchild is
//!    the surviving daemon and is no longer a session leader (so it
//!    cannot reacquire a controlling terminal by accident).
//! 5. Grandchild redirects stdin/stdout to `/dev/null`, writes
//!    [`STARTUP_OK`] back to the parent, redirects stderr too, then
//!    returns to the caller.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::path::Path;

use nix::sys::wait::waitpid;
use nix::unistd::{ForkResult, Pid, fork, setsid};

use super::DetachError;

const STARTUP_OK: u8 = 1;
/// Followed by a UTF-8 message and `_exit(1)`.
const STARTUP_FAIL: u8 = 0;

pub fn detach(log_dir: &Path) -> Result<(), DetachError> {
  // Pipe must be created before the fork so both sides inherit the
  // ends. Owned FDs let RAII clean up on the error paths.
  let (reader, writer) = nix::unistd::pipe().map_err(syscall_error("pipe"))?;

  match unsafe { fork() }.map_err(syscall_error("fork"))? {
    ForkResult::Parent { child } => {
      parent_after_first_fork(reader, writer, child)?;
      unreachable!("parent exited above");
    },
    ForkResult::Child => {
      child_after_first_fork(reader, writer, log_dir)?;
      Ok(())
    },
  }
}

fn parent_after_first_fork(reader: OwnedFd, writer: OwnedFd, child: Pid) -> Result<(), DetachError> {
  drop(writer);

  // Reap the intermediate child. The grandchild is re-parented to
  // init and we never wait on it; not reaping the intermediate would
  // leak a zombie until the parent shell exits.
  let _ = waitpid(child, None);

  let mut reader: std::fs::File = reader.into();
  let mut first = [0u8; 1];
  let n = reader.read(&mut first).map_err(io_error("read handshake"))?;
  if n == 0 {
    return Err(DetachError::ChildReportedFailure {
      message: "child closed the handshake pipe without writing".to_string(),
    });
  }
  match first[0] {
    STARTUP_OK => {
      std::process::exit(0);
    },
    STARTUP_FAIL => {
      let mut rest = String::new();
      let _ = reader.read_to_string(&mut rest);
      Err(DetachError::ChildReportedFailure {
        message: rest.trim().to_string(),
      })
    },
    other => Err(DetachError::ChildReportedFailure {
      message: format!("unrecognized handshake byte 0x{other:02x}"),
    }),
  }
}

fn child_after_first_fork(reader: OwnedFd, writer: OwnedFd, log_dir: &Path) -> Result<(), DetachError> {
  drop(reader);

  setsid().map_err(syscall_error("setsid"))?;

  match unsafe { fork() }.map_err(syscall_error("fork2"))? {
    ForkResult::Parent { .. } => {
      drop(writer);
      // `_exit` (not `exit`) so atexit handlers from the original
      // process do not run — they could touch shared state or stdio
      // we are about to redirect.
      // Safety: `_exit` is async-signal-safe and never returns.
      unsafe { libc::_exit(0) };
    },
    ForkResult::Child => grandchild_setup(writer, log_dir),
  }
}

fn grandchild_setup(writer: OwnedFd, _log_dir: &Path) -> Result<(), DetachError> {
  let writer_fd = writer.as_raw_fd();

  // The intermediate process is the session leader. When it `_exit`s
  // the kernel sends SIGHUP to every process still in the session.
  // The default disposition is "terminate" — we would die before
  // returning. Set SIG_IGN early; the tokio signal handler installed
  // later replaces this with the proper logger.
  if let Err(err) = ignore_sighup() {
    report_failure_and_exit(writer_fd, &format!("{err}"));
  }

  // stderr is intentionally left open through the handshake so a
  // syscall failure here can surface to the operator's terminal even
  // though the pipe carries the structured message.
  if let Err(err) = redirect_pre_handshake_stdio() {
    report_failure_and_exit(writer_fd, &format!("{err}"));
  }

  let mut w: std::fs::File = writer.into();
  if let Err(err) = w.write_all(&[STARTUP_OK]) {
    let _ = err;
  }
  drop(w);

  // Now that the operator's terminal has seen the parent exit, no
  // one is reading the original stderr. Redirect to /dev/null. A
  // failure here is best-effort: the daemon still works, it just
  // leaks bytes to a terminal nobody is watching.
  if let Err(err) = redirect_stderr_to_devnull() {
    let _ = err;
  }

  Ok(())
}

/// Best-effort failure path: write the diagnostic down the handshake
/// pipe, then `_exit` so atexit handlers and Drop on Vik state do not
/// run inside a half-initialized daemon.
fn report_failure_and_exit(writer_fd: std::os::fd::RawFd, message: &str) -> ! {
  // Safety: ownership of the FD is borrowed only for the lifetime of
  // this function; `forget` prevents Rust's drop from closing it
  // before `_exit` tears the process down.
  let mut file = unsafe { std::fs::File::from_raw_fd(writer_fd) };
  let _ = file.write_all(&[STARTUP_FAIL]);
  let _ = file.write_all(message.as_bytes());
  let _ = file.flush();
  std::mem::forget(file);
  // Safety: `_exit` is async-signal-safe and never returns.
  unsafe { libc::_exit(1) };
}

fn redirect_pre_handshake_stdio() -> Result<(), DetachError> {
  use std::fs::OpenOptions;

  let devnull_in = OpenOptions::new()
    .read(true)
    .open("/dev/null")
    .map_err(io_error("open /dev/null for stdin"))?;
  dup2_raw(devnull_in.as_raw_fd(), libc::STDIN_FILENO, "dup2 stdin")?;
  drop(devnull_in);

  let devnull_out = OpenOptions::new()
    .write(true)
    .open("/dev/null")
    .map_err(io_error("open /dev/null for stdout"))?;
  dup2_raw(devnull_out.as_raw_fd(), libc::STDOUT_FILENO, "dup2 stdout")?;
  drop(devnull_out);

  Ok(())
}

fn redirect_stderr_to_devnull() -> Result<(), DetachError> {
  use std::fs::OpenOptions;

  let devnull_err = OpenOptions::new()
    .write(true)
    .open("/dev/null")
    .map_err(io_error("open /dev/null for stderr"))?;
  dup2_raw(devnull_err.as_raw_fd(), libc::STDERR_FILENO, "dup2 stderr")?;
  drop(devnull_err);

  Ok(())
}

fn ignore_sighup() -> Result<(), DetachError> {
  // Safety: `signal` is POSIX; SIG_IGN is always a valid disposition.
  let rc = unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
  if rc == libc::SIG_ERR {
    return Err(DetachError::Syscall {
      step: "signal SIGHUP",
      source: std::io::Error::last_os_error(),
    });
  }
  Ok(())
}

/// `nix 0.31` switched `dup2` to an `AsFd`-based API that won't aim
/// two descriptors at one file without cloning the underlying handle.
/// Falling through to the raw libc call is simpler than fighting the
/// type system here.
fn dup2_raw(old: std::os::fd::RawFd, new: std::os::fd::RawFd, step: &'static str) -> Result<(), DetachError> {
  // Safety: `dup2` is POSIX; both FDs are valid integers in the
  // child process.
  let rc = unsafe { libc::dup2(old, new) };
  if rc == -1 {
    return Err(DetachError::Syscall {
      step,
      source: std::io::Error::last_os_error(),
    });
  }
  Ok(())
}

fn syscall_error(step: &'static str) -> impl FnOnce(nix::errno::Errno) -> DetachError {
  move |errno| DetachError::Syscall {
    step,
    source: std::io::Error::from_raw_os_error(errno as i32),
  }
}

fn io_error(step: &'static str) -> impl FnOnce(std::io::Error) -> DetachError {
  move |source| DetachError::Syscall { step, source }
}

#[allow(dead_code)]
fn owned_to_raw(fd: OwnedFd) -> std::os::fd::RawFd {
  fd.into_raw_fd()
}
