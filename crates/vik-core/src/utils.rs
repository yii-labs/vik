use crate::{BlockerRef, Issue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Posix,
    Windows,
}

impl HostPlatform {
    pub fn current() -> Self {
        current_host_platform()
    }
}

#[cfg(windows)]
fn current_host_platform() -> HostPlatform {
    HostPlatform::Windows
}

#[cfg(not(windows))]
fn current_host_platform() -> HostPlatform {
    HostPlatform::Posix
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosixShell {
    Bash,
    Sh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellInvocation<'a> {
    program: &'static str,
    args: &'static [&'static str],
    command: &'a str,
}

impl<'a> ShellInvocation<'a> {
    pub fn for_current_platform(command: &'a str, posix_shell: PosixShell) -> Self {
        Self::for_platform(command, HostPlatform::current(), posix_shell)
    }

    pub fn for_platform(command: &'a str, platform: HostPlatform, posix_shell: PosixShell) -> Self {
        match platform {
            HostPlatform::Posix => match posix_shell {
                PosixShell::Bash => Self {
                    program: "bash",
                    args: &["-lc"],
                    command,
                },
                PosixShell::Sh => Self {
                    program: "sh",
                    args: &["-lc"],
                    command,
                },
            },
            HostPlatform::Windows => Self {
                program: "powershell.exe",
                args: &["-NoProfile", "-NonInteractive", "-Command"],
                command,
            },
        }
    }

    pub fn program(&self) -> &'static str {
        self.program
    }

    pub fn args(&self) -> &'static [&'static str] {
        self.args
    }

    pub fn command(&self) -> &'a str {
        self.command
    }
}

pub fn normalize_state(state: &str) -> String {
    state.to_lowercase()
}

pub fn sanitize_workspace_key(identifier: &str) -> String {
    identifier
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub fn session_id(thread_id: &str, turn_id: &str) -> String {
    if is_uuid_like(thread_id) && is_uuid_like(turn_id) {
        return thread_id.to_string();
    }

    format!("{thread_id}-{turn_id}")
}

fn is_uuid_like(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }

    value.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_hexdigit(),
    })
}

pub fn issue_is_active(
    issue: &Issue,
    active_states: &[String],
    terminal_states: &[String],
) -> bool {
    let state = normalize_state(&issue.state);
    active_states.iter().any(|s| normalize_state(s) == state)
        && !terminal_states.iter().any(|s| normalize_state(s) == state)
}

pub fn blocker_is_terminal(blocker: &BlockerRef, terminal_states: &[String]) -> bool {
    blocker
        .state
        .as_deref()
        .map(|state| {
            let state = normalize_state(state);
            terminal_states
                .iter()
                .any(|terminal| normalize_state(terminal) == state)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use crate::AgentSession;

    use super::{HostPlatform, PosixShell, ShellInvocation, session_id};

    #[test]
    fn shell_invocation_uses_bash_for_posix_app_server() {
        let invocation = ShellInvocation::for_platform(
            "codex app-server",
            HostPlatform::Posix,
            PosixShell::Bash,
        );
        assert_eq!(invocation.program(), "bash");
        assert_eq!(invocation.args(), &["-lc"]);
        assert_eq!(invocation.command(), "codex app-server");
    }

    #[test]
    fn shell_invocation_uses_sh_for_posix_hooks() {
        let invocation =
            ShellInvocation::for_platform("echo hook", HostPlatform::Posix, PosixShell::Sh);
        assert_eq!(invocation.program(), "sh");
        assert_eq!(invocation.args(), &["-lc"]);
        assert_eq!(invocation.command(), "echo hook");
    }

    #[test]
    fn shell_invocation_uses_powershell_for_windows_hooks() {
        let invocation = ShellInvocation::for_platform(
            "Write-Output hook",
            HostPlatform::Windows,
            PosixShell::Sh,
        );
        assert_eq!(invocation.program(), "powershell.exe");
        assert_eq!(
            invocation.args(),
            &["-NoProfile", "-NonInteractive", "-Command"]
        );
        assert_eq!(invocation.command(), "Write-Output hook");
    }

    #[test]
    fn session_id_uses_thread_uuid_for_codex_uuid_ids() {
        let thread_id = "019dfab1-fd48-78c0-9b40-cf507bd19842";
        let turn_id = "019dfab1-fd58-7a21-8285-58d94bbb614f";

        assert_eq!(session_id(thread_id, turn_id), thread_id);
    }

    #[test]
    fn session_id_preserves_composite_for_non_uuid_ids() {
        assert_eq!(session_id("thread-1", "turn-2"), "thread-1-turn-2");
    }

    #[test]
    fn session_id_preserves_composite_when_only_thread_id_is_uuid() {
        let thread_id = "019dfab1-fd48-78c0-9b40-cf507bd19842";

        assert_eq!(
            session_id(thread_id, "turn-2"),
            format!("{thread_id}-turn-2")
        );
    }

    #[test]
    fn session_id_preserves_composite_when_only_turn_id_is_uuid() {
        let turn_id = "019dfab1-fd58-7a21-8285-58d94bbb614f";

        assert_eq!(
            session_id("thread-1", turn_id),
            format!("thread-1-{turn_id}")
        );
    }

    #[test]
    fn agent_session_uses_thread_uuid_for_codex_uuid_ids() {
        let thread_id = "019dfab1-fd48-78c0-9b40-cf507bd19842";
        let turn_id = "019dfab1-fd58-7a21-8285-58d94bbb614f";
        let session = AgentSession::new(thread_id, turn_id);

        assert_eq!(session.session_id, thread_id);
        assert_eq!(session.thread_id, thread_id);
        assert_eq!(session.turn_id, turn_id);
    }
}
