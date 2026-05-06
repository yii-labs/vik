use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vik_core::{HostPlatform, WorkflowDefinition};
use vik_tracker::{
    CommonTrackerConfig, GitHubTrackerConfig, LinearTrackerConfig, TrackerConfig,
    TrackerFilterConfig,
};

use crate::WorkflowError;
use crate::yaml::{
    concurrency_map, expand_path_value, get_map, i64_value, json_value, nested_map,
    resolve_exact_env, string_value, string_vec, u32_value, u64_value, usize_value,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollingConfig {
    pub interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HooksConfig {
    pub after_create: Option<String>,
    pub before_run: Option<String>,
    pub after_run: Option<String>,
    pub before_remove: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub runtime: AgentRuntimeConfig,
    pub max_concurrent_agents: usize,
    pub max_turns: u32,
    pub max_retry_backoff_ms: u64,
    pub max_concurrent_agents_by_state: HashMap<String, usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntimeConfig {
    #[default]
    Codex,
}

impl AgentRuntimeConfig {
    fn from_name(raw: &str) -> Result<Self, WorkflowError> {
        match raw.trim() {
            "codex" => Ok(Self::Codex),
            "" => Err(WorkflowError::InvalidConfig(
                "agent.runtime is empty".to_string(),
            )),
            value => Err(WorkflowError::InvalidConfig(format!(
                "unsupported agent.runtime: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexConfig {
    pub command: String,
    pub model: Option<String>,
    pub model_reasoning_effort: Option<String>,
    pub approval_policy: Option<serde_json::Value>,
    pub approvals_reviewer: Option<serde_json::Value>,
    pub thread_sandbox: Option<serde_json::Value>,
    pub turn_sandbox_policy: Option<serde_json::Value>,
    pub turn_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub stall_timeout_ms: i64,
}

impl CodexConfig {
    pub fn has_model_cli_config(&self) -> bool {
        self.model
            .as_deref()
            .is_some_and(|model| !model.trim().is_empty())
            || self
                .model_reasoning_effort
                .as_deref()
                .is_some_and(|effort| !effort.trim().is_empty())
    }

    pub fn split_command_at_app_server(&self) -> Option<(&str, &str)> {
        let command = self.command.trim();
        find_shell_token_start(command, "app-server").map(|index| command.split_at(index))
    }

    pub fn command_program(&self) -> Option<String> {
        self.command_program_for_platform(HostPlatform::current())
    }

    pub fn command_program_for_platform(&self, platform: HostPlatform) -> Option<String> {
        match platform {
            HostPlatform::Posix => first_posix_command_program(&self.command),
            HostPlatform::Windows => split_windows_command_line(&self.command).into_iter().next(),
        }
    }
}

fn first_posix_command_program(command: &str) -> Option<String> {
    let mut rest = command.trim();
    loop {
        let token = first_shell_token(rest)?;
        if !is_shell_env_assignment(&token) {
            return Some(token);
        }
        rest = drop_first_shell_token(rest);
        if rest.is_empty() {
            return None;
        }
    }
}

pub(crate) fn first_shell_token(command: &str) -> Option<String> {
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;

    for ch in command.trim().chars() {
        if escaped {
            token.push(ch);
            escaped = false;
            started = true;
            continue;
        }

        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    token.push(ch);
                    started = true;
                }
            }
            Some('"') => {
                if ch == '"' {
                    quote = None;
                } else if ch == '\\' {
                    escaped = true;
                } else {
                    token.push(ch);
                    started = true;
                }
            }
            Some(_) => unreachable!(),
            None => {
                if ch.is_whitespace() {
                    if started {
                        return (!token.is_empty()).then_some(token);
                    }
                } else if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                    started = true;
                } else if ch == '\\' {
                    escaped = true;
                    started = true;
                } else {
                    token.push(ch);
                    started = true;
                }
            }
        }
    }

    if escaped {
        token.push('\\');
    }
    (started && !token.is_empty()).then_some(token)
}

pub(crate) fn drop_first_shell_token(input: &str) -> &str {
    let input = input.trim_start();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;

    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            started = true;
            continue;
        }

        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    started = true;
                }
            }
            Some('"') => {
                if ch == '"' {
                    quote = None;
                } else if ch == '\\' {
                    escaped = true;
                } else {
                    started = true;
                }
            }
            Some(_) => unreachable!(),
            None => {
                if ch.is_whitespace() {
                    if started {
                        return input[index..].trim_start();
                    }
                } else if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                    started = true;
                } else if ch == '\\' {
                    escaped = true;
                    started = true;
                } else {
                    started = true;
                }
            }
        }
    }

    ""
}

pub(crate) fn is_shell_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !name.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

fn find_shell_token_start(command: &str, needle: &str) -> Option<usize> {
    let mut token_start = None;
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;

    for (index, ch) in command.char_indices() {
        if token_start.is_none() {
            if ch.is_whitespace() {
                continue;
            }
            token_start = Some(index);
        }

        if escaped {
            token.push(ch);
            escaped = false;
            continue;
        }

        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    token.push(ch);
                }
            }
            Some('"') => {
                if ch == '"' {
                    quote = None;
                } else if ch == '\\' {
                    escaped = true;
                } else {
                    token.push(ch);
                }
            }
            Some(_) => unreachable!(),
            None => {
                if ch.is_whitespace() {
                    if token == needle {
                        return token_start;
                    }
                    token_start = None;
                    token.clear();
                } else if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                } else if ch == '\\' {
                    escaped = true;
                } else {
                    token.push(ch);
                }
            }
        }
    }

    if escaped {
        token.push('\\');
    }
    if token == needle { token_start } else { None }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub workflow_path: PathBuf,
    pub tracker: TrackerConfig,
    pub polling: PollingConfig,
    pub workspace: WorkspaceConfig,
    pub logging: LoggingConfig,
    pub hooks: HooksConfig,
    pub agent: AgentConfig,
    pub codex: CodexConfig,
    pub server: Option<ServerConfig>,
}

impl ServiceConfig {
    pub fn from_definition(definition: &WorkflowDefinition) -> Result<Self, WorkflowError> {
        let workflow_dir = definition
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let tracker_map = get_map(&definition.config, "tracker");
        let polling_map = get_map(&definition.config, "polling");
        let workspace_map = get_map(&definition.config, "workspace");
        let logging_map = get_map(&definition.config, "logging");
        let hooks_map = get_map(&definition.config, "hooks");
        let agent_map = get_map(&definition.config, "agent");
        let codex_map = get_map(&definition.config, "codex");
        let server_map = get_map(&definition.config, "server");

        let tracker_kind = string_value(tracker_map, "kind").unwrap_or_default();
        let project_slug = string_value(tracker_map, "project_slug").unwrap_or_default();
        let repository = string_value(tracker_map, "repository").unwrap_or_default();
        let active_states = string_vec(tracker_map, "active_states")
            .unwrap_or_else(|| vec!["Todo".to_string(), "In Progress".to_string()]);
        let terminal_states = string_vec(tracker_map, "terminal_states").unwrap_or_else(|| {
            vec![
                "Closed".to_string(),
                "Cancelled".to_string(),
                "Canceled".to_string(),
                "Duplicate".to_string(),
                "Done".to_string(),
            ]
        });
        let tracker_filter_map = nested_map(tracker_map, "filter");
        let filter = TrackerFilterConfig {
            assignees: string_vec(tracker_filter_map, "assignees").unwrap_or_default(),
            tags: string_vec(tracker_filter_map, "tags").unwrap_or_default(),
        };
        let common_tracker = CommonTrackerConfig {
            active_states,
            terminal_states,
            filter,
        };
        let tracker = match tracker_kind.as_str() {
            "linear" => TrackerConfig::linear(
                common_tracker,
                LinearTrackerConfig::new(
                    provider_endpoint(tracker_map, LinearTrackerConfig::default_endpoint()),
                    provider_api_key(tracker_map, LinearTrackerConfig::api_key_from_env)?,
                    project_slug,
                ),
            ),
            "github" => TrackerConfig::github(
                common_tracker,
                GitHubTrackerConfig::new(
                    provider_endpoint(tracker_map, GitHubTrackerConfig::default_endpoint()),
                    provider_api_key(tracker_map, GitHubTrackerConfig::api_key_from_env)?,
                    repository,
                ),
            ),
            _ => TrackerConfig::unsupported(common_tracker, tracker_kind),
        };

        let workspace_root = string_value(workspace_map, "root")
            .map(|raw| expand_path_value(&raw, &workflow_dir))
            .transpose()?
            .unwrap_or_else(|| env::temp_dir().join("vik_workspaces"));
        let logging_dir = string_value(logging_map, "dir")
            .map(|raw| expand_path_value(&raw, &workflow_dir))
            .transpose()?
            .unwrap_or_else(|| workspace_root.join("logs"));

        let hooks = HooksConfig {
            after_create: string_value(hooks_map, "after_create"),
            before_run: string_value(hooks_map, "before_run"),
            after_run: string_value(hooks_map, "after_run"),
            before_remove: string_value(hooks_map, "before_remove"),
            timeout_ms: u64_value(hooks_map, "timeout_ms").unwrap_or(60_000),
        };
        if hooks.timeout_ms == 0 {
            return Err(WorkflowError::InvalidConfig(
                "hooks.timeout_ms must be positive".to_string(),
            ));
        }

        let max_concurrent_agents = usize_value(agent_map, "max_concurrent_agents").unwrap_or(10);
        let max_turns = u32_value(agent_map, "max_turns").unwrap_or(20);
        if max_turns == 0 {
            return Err(WorkflowError::InvalidConfig(
                "agent.max_turns must be positive".to_string(),
            ));
        }
        let max_retry_backoff_ms = u64_value(agent_map, "max_retry_backoff_ms").unwrap_or(300_000);
        let max_concurrent_agents_by_state =
            concurrency_map(agent_map, "max_concurrent_agents_by_state");
        let runtime = string_value(agent_map, "runtime")
            .map(|raw| AgentRuntimeConfig::from_name(&raw))
            .transpose()?
            .unwrap_or_default();

        let codex = CodexConfig {
            command: string_value(codex_map, "command")
                .unwrap_or_else(|| "codex app-server".to_string()),
            model: string_value(codex_map, "model"),
            model_reasoning_effort: string_value(codex_map, "model_reasoning_effort"),
            approval_policy: json_value(codex_map, "approval_policy"),
            approvals_reviewer: json_value(codex_map, "approvals_reviewer"),
            thread_sandbox: json_value(codex_map, "thread_sandbox"),
            turn_sandbox_policy: json_value(codex_map, "turn_sandbox_policy"),
            turn_timeout_ms: u64_value(codex_map, "turn_timeout_ms").unwrap_or(3_600_000),
            read_timeout_ms: u64_value(codex_map, "read_timeout_ms").unwrap_or(30_000),
            stall_timeout_ms: i64_value(codex_map, "stall_timeout_ms").unwrap_or(300_000),
        };

        let server = server_map
            .and_then(|map| u64_value(Some(map), "port"))
            .map(|port| ServerConfig { port: port as u16 });

        Ok(Self {
            workflow_path: definition.path.clone(),
            tracker,
            polling: PollingConfig {
                interval_ms: u64_value(polling_map, "interval_ms").unwrap_or(30_000),
            },
            workspace: WorkspaceConfig {
                root: workspace_root,
            },
            logging: LoggingConfig { dir: logging_dir },
            hooks,
            agent: AgentConfig {
                runtime,
                max_concurrent_agents,
                max_turns,
                max_retry_backoff_ms,
                max_concurrent_agents_by_state,
            },
            codex,
            server,
        })
    }

    pub fn validate_for_dispatch(&self) -> Result<(), WorkflowError> {
        self.tracker.validate()?;
        self.validate_non_tracker_config()
    }

    pub(crate) fn validate_non_tracker_config(&self) -> Result<(), WorkflowError> {
        if self.polling.interval_ms == 0 {
            return Err(WorkflowError::InvalidConfig(
                "polling.interval_ms must be positive".to_string(),
            ));
        }
        match self.agent.runtime {
            AgentRuntimeConfig::Codex => self.validate_codex_config()?,
        }
        Ok(())
    }

    pub(crate) fn validate_codex_config(&self) -> Result<(), WorkflowError> {
        if self.codex.command.trim().is_empty() {
            return Err(WorkflowError::InvalidConfig(
                "codex.command is empty".to_string(),
            ));
        }
        if self
            .codex
            .model
            .as_ref()
            .is_some_and(|model| model.trim().is_empty())
        {
            return Err(WorkflowError::InvalidConfig(
                "codex.model is empty".to_string(),
            ));
        }
        if self
            .codex
            .model_reasoning_effort
            .as_ref()
            .is_some_and(|effort| effort.trim().is_empty())
        {
            return Err(WorkflowError::InvalidConfig(
                "codex.model_reasoning_effort is empty".to_string(),
            ));
        }
        if self.codex.has_model_cli_config() && self.codex.split_command_at_app_server().is_none() {
            return Err(WorkflowError::InvalidConfig(
                "codex.command must include app-server when codex.model or codex.model_reasoning_effort is set".to_string(),
            ));
        }
        Ok(())
    }
}

fn provider_endpoint(tracker_map: Option<&serde_yaml::Mapping>, default_endpoint: &str) -> String {
    string_value(tracker_map, "endpoint").unwrap_or_else(|| default_endpoint.to_string())
}

fn provider_api_key(
    tracker_map: Option<&serde_yaml::Mapping>,
    from_env: fn() -> Option<String>,
) -> Result<String, WorkflowError> {
    Ok(string_value(tracker_map, "api_key")
        .or_else(from_env)
        .map(resolve_exact_env)
        .transpose()?
        .unwrap_or_default())
}
