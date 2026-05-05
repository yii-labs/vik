use vik_core::{HostPlatform, PosixShell, ShellInvocation};
use vik_workflow::CodexConfig;

use crate::codex::process::ProcessCommand;

pub(crate) fn codex_spawn_command(config: &CodexConfig) -> String {
    let args = codex_model_config_shell_args(config);
    if args.is_empty() {
        return config.command.clone();
    }

    let joined_args = args.join(" ");
    if let Some((prefix, app_server_command)) = config.split_command_at_app_server() {
        let prefix = prefix.trim_end();
        let app_server_command = app_server_command.trim_start();
        if prefix.is_empty() {
            format!("{joined_args} {app_server_command}")
        } else {
            format!("{prefix} {joined_args} {app_server_command}")
        }
    } else {
        let command = config.command.trim();
        format!("{command} {joined_args}")
    }
}

pub(crate) fn codex_spawn_process_command(config: &CodexConfig) -> ProcessCommand {
    codex_spawn_process_command_for_platform(config, HostPlatform::current())
}

pub(crate) fn codex_spawn_process_command_for_platform(
    config: &CodexConfig,
    platform: HostPlatform,
) -> ProcessCommand {
    match platform {
        HostPlatform::Posix => {
            let command = codex_spawn_command(config);
            let shell =
                ShellInvocation::for_platform(&command, HostPlatform::Posix, PosixShell::Bash);
            ProcessCommand::new(
                shell.program(),
                shell
                    .args()
                    .iter()
                    .copied()
                    .chain(std::iter::once(shell.command())),
            )
        }
        HostPlatform::Windows => codex_spawn_direct_command(config),
    }
}

fn codex_spawn_direct_command(config: &CodexConfig) -> ProcessCommand {
    let mut argv = split_windows_command_line(&config.command);
    if argv.is_empty() {
        return ProcessCommand::new(config.command.trim(), std::iter::empty::<String>());
    }

    let model_args = codex_model_config_argv(config);
    if !model_args.is_empty() {
        let insert_at = argv
            .iter()
            .position(|arg| arg == "app-server")
            .unwrap_or(argv.len());
        argv.splice(insert_at..insert_at, model_args);
    }

    let program = argv.remove(0);
    ProcessCommand::new(program, argv)
}

fn codex_model_config_shell_args(config: &CodexConfig) -> Vec<String> {
    codex_model_config_argv(config)
        .chunks_exact(2)
        .map(|pair| format!("{} {}", pair[0], shell_single_quote(&pair[1])))
        .collect()
}

fn codex_model_config_argv(config: &CodexConfig) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = config.model.as_deref().map(str::trim)
        && !model.is_empty()
    {
        args.push("--config".to_string());
        args.push(format!("model={}", toml_string(model)));
    }
    if let Some(effort) = config.model_reasoning_effort.as_deref().map(str::trim)
        && !effort.is_empty()
    {
        args.push("--config".to_string());
        args.push(format!("model_reasoning_effort={effort}"));
    }
    args
}

fn split_windows_command_line(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut arg_started = false;
    let mut backslashes = 0;

    for ch in input.trim().chars() {
        match ch {
            '\\' => {
                backslashes += 1;
                arg_started = true;
            }
            '"' => {
                arg_started = true;
                current.extend(std::iter::repeat_n('\\', backslashes / 2));
                if backslashes % 2 == 0 {
                    in_quotes = !in_quotes;
                } else {
                    current.push('"');
                }
                backslashes = 0;
            }
            ch if ch.is_whitespace() && !in_quotes => {
                current.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                if arg_started {
                    args.push(std::mem::take(&mut current));
                    arg_started = false;
                }
            }
            _ => {
                current.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                current.push(ch);
                arg_started = true;
            }
        }
    }

    current.extend(std::iter::repeat_n('\\', backslashes));
    if arg_started {
        args.push(current);
    }
    args
}

fn toml_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
