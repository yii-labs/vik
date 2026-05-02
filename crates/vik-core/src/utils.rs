use crate::{BlockerRef, Issue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosixShell {
    Bash,
    Sh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Posix,
    Windows,
}

impl HostPlatform {
    #[cfg(windows)]
    pub fn current() -> Self {
        Self::Windows
    }

    #[cfg(not(windows))]
    pub fn current() -> Self {
        Self::Posix
    }
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
            HostPlatform::Windows => Self {
                program: "powershell.exe",
                args: &["-NoProfile", "-NonInteractive", "-Command"],
                command,
            },
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
    format!("{thread_id}-{turn_id}")
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
    use super::{HostPlatform, PosixShell, ShellInvocation};

    #[test]
    fn shell_invocation_uses_powershell_on_windows() {
        let shell = ShellInvocation::for_platform(
            "codex app-server",
            HostPlatform::Windows,
            PosixShell::Bash,
        );

        assert_eq!(shell.program(), "powershell.exe");
        assert_eq!(shell.args(), &["-NoProfile", "-NonInteractive", "-Command"]);
        assert_eq!(shell.command(), "codex app-server");
    }

    #[test]
    fn shell_invocation_preserves_posix_shell_choice() {
        let bash = ShellInvocation::for_platform(
            "codex app-server",
            HostPlatform::Posix,
            PosixShell::Bash,
        );
        let sh = ShellInvocation::for_platform("git status .", HostPlatform::Posix, PosixShell::Sh);

        assert_eq!(bash.program(), "bash");
        assert_eq!(bash.args(), &["-lc"]);
        assert_eq!(bash.command(), "codex app-server");
        assert_eq!(sh.program(), "sh");
        assert_eq!(sh.args(), &["-lc"]);
        assert_eq!(sh.command(), "git status .");
    }
}
