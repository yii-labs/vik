use thiserror::Error;

use crate::{BlockerRef, Issue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Posix,
    Windows,
}

impl HostPlatform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Posix
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosixShell {
    Bash,
    Sh,
}

impl PosixShell {
    fn program(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Sh => "sh",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInvocation {
    program: String,
    args: Vec<String>,
}

impl CommandInvocation {
    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn codex_app_server(
        platform: HostPlatform,
        command: &str,
    ) -> Result<Self, CommandParseError> {
        match platform {
            HostPlatform::Posix => Ok(Self::posix_shell(PosixShell::Bash, command)),
            HostPlatform::Windows => Self::windows_command_line(command),
        }
    }

    pub fn hook_script(platform: HostPlatform, script: &str) -> Self {
        match platform {
            HostPlatform::Posix => Self::posix_shell(PosixShell::Sh, script),
            HostPlatform::Windows => Self {
                program: "powershell.exe".to_string(),
                args: vec![
                    "-NoLogo".to_string(),
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-Command".to_string(),
                    script.to_string(),
                ],
            },
        }
    }

    pub fn posix_shell(shell: PosixShell, script: &str) -> Self {
        Self {
            program: shell.program().to_string(),
            args: vec!["-lc".to_string(), script.to_string()],
        }
    }

    pub fn windows_command_line(command: &str) -> Result<Self, CommandParseError> {
        let mut argv = windows_command_argv(command)?.into_iter();
        let program = argv.next().ok_or(CommandParseError::Empty)?;
        Ok(Self {
            program,
            args: argv.collect(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandParseError {
    #[error("command is empty")]
    Empty,
    #[error("unterminated {0} quote")]
    UnterminatedQuote(char),
}

pub fn windows_command_argv(command: &str) -> Result<Vec<String>, CommandParseError> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut in_arg = false;

    for ch in command.chars() {
        match quote {
            Some(active_quote) => {
                if ch == active_quote {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None if ch.is_whitespace() => {
                if in_arg {
                    args.push(std::mem::take(&mut current));
                    in_arg = false;
                }
            }
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                in_arg = true;
            }
            None => {
                current.push(ch);
                in_arg = true;
            }
        }
    }

    if let Some(active_quote) = quote {
        return Err(CommandParseError::UnterminatedQuote(active_quote));
    }
    if in_arg {
        args.push(current);
    }
    if args.is_empty() {
        return Err(CommandParseError::Empty);
    }
    Ok(args)
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
    use super::*;

    #[test]
    fn shell_invocation_keeps_posix_app_server_on_bash() {
        let invocation =
            CommandInvocation::codex_app_server(HostPlatform::Posix, "codex app-server").unwrap();

        assert_eq!(invocation.program(), "bash");
        assert_eq!(invocation.args(), ["-lc", "codex app-server"]);
    }

    #[test]
    fn shell_invocation_uses_direct_windows_app_server_argv() {
        let invocation = CommandInvocation::codex_app_server(
            HostPlatform::Windows,
            r#"codex --config 'model="gpt-5.5"' app-server"#,
        )
        .unwrap();

        assert_eq!(invocation.program(), "codex");
        assert_eq!(
            invocation.args(),
            ["--config", "model=\"gpt-5.5\"", "app-server"]
        );
    }

    #[test]
    fn shell_invocation_preserves_unquoted_windows_absolute_path() {
        let invocation = CommandInvocation::windows_command_line(
            r#"C:\Users\runner\bin\codex.exe --config key=value app-server"#,
        )
        .unwrap();

        assert_eq!(invocation.program(), r#"C:\Users\runner\bin\codex.exe"#);
        assert_eq!(invocation.args(), ["--config", "key=value", "app-server"]);
    }

    #[test]
    fn shell_invocation_preserves_quoted_windows_absolute_path() {
        let invocation = CommandInvocation::windows_command_line(
            r#""C:\Program Files\Codex\codex.exe" app-server"#,
        )
        .unwrap();

        assert_eq!(invocation.program(), r#"C:\Program Files\Codex\codex.exe"#);
        assert_eq!(invocation.args(), ["app-server"]);
    }

    #[test]
    fn shell_invocation_uses_platform_hook_shells() {
        let posix = CommandInvocation::hook_script(HostPlatform::Posix, "git status .");
        assert_eq!(posix.program(), "sh");
        assert_eq!(posix.args(), ["-lc", "git status ."]);

        let windows = CommandInvocation::hook_script(HostPlatform::Windows, "git status .");
        assert_eq!(windows.program(), "powershell.exe");
        assert_eq!(
            windows.args(),
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "git status ."
            ]
        );
    }

    #[test]
    fn shell_invocation_rejects_empty_windows_command() {
        assert_eq!(
            CommandInvocation::windows_command_line("   ").unwrap_err(),
            CommandParseError::Empty
        );
    }
}
