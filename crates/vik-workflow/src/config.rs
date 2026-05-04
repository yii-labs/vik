use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vik_core::WorkflowDefinition;

use crate::WorkflowError;
use crate::yaml::{
    concurrency_map, expand_path_value, get_map, i64_value, json_value, nested_map,
    resolve_exact_env, string_value, string_vec, u32_value, u64_value, usize_value,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerConfig {
    pub kind: String,
    pub endpoint: String,
    pub api_key: String,
    pub project_slug: String,
    pub active_states: Vec<String>,
    pub terminal_states: Vec<String>,
    #[serde(default)]
    pub filter: TrackerFilterConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerFilterConfig {
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

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
    pub service_dir: PathBuf,
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
    pub max_concurrent_agents: usize,
    pub max_turns: u32,
    pub max_retry_backoff_ms: u64,
    pub max_concurrent_agents_by_state: HashMap<String, usize>,
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
        let endpoint = string_value(tracker_map, "endpoint").unwrap_or_else(|| {
            if tracker_kind == "linear" {
                "https://api.linear.app/graphql".to_string()
            } else {
                String::new()
            }
        });
        let api_key = string_value(tracker_map, "api_key")
            .or_else(|| env::var("LINEAR_API_KEY").ok())
            .map(resolve_exact_env)
            .transpose()?
            .unwrap_or_default();
        let project_slug = string_value(tracker_map, "project_slug").unwrap_or_default();
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

        let workspace_root = string_value(workspace_map, "root")
            .map(|raw| expand_path_value(&raw, &workflow_dir))
            .transpose()?
            .unwrap_or_else(|| env::temp_dir().join("vik_workspaces"));
        let logging_dir = string_value(logging_map, "dir")
            .map(|raw| expand_path_value(&raw, &workflow_dir))
            .transpose()?
            .unwrap_or_else(|| workspace_root.join(".vik").join("logs"));
        let service_logging_dir = service_logging_dir_from_definition(definition)?
            .unwrap_or_else(|| default_service_logging_dir(&workflow_dir));

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
            tracker: TrackerConfig {
                kind: tracker_kind,
                endpoint,
                api_key,
                project_slug,
                active_states,
                terminal_states,
                filter,
            },
            polling: PollingConfig {
                interval_ms: u64_value(polling_map, "interval_ms").unwrap_or(30_000),
            },
            workspace: WorkspaceConfig {
                root: workspace_root,
            },
            logging: LoggingConfig {
                dir: logging_dir,
                service_dir: service_logging_dir,
            },
            hooks,
            agent: AgentConfig {
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
        if self.tracker.kind != "linear" {
            return Err(WorkflowError::UnsupportedTrackerKind);
        }
        if self.tracker.api_key.trim().is_empty() {
            return Err(WorkflowError::MissingTrackerApiKey);
        }
        if self.tracker.project_slug.trim().is_empty() {
            return Err(WorkflowError::MissingTrackerProjectSlug);
        }
        if self.polling.interval_ms == 0 {
            return Err(WorkflowError::InvalidConfig(
                "polling.interval_ms must be positive".to_string(),
            ));
        }
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
pub fn service_logging_dir_from_definition(
    definition: &WorkflowDefinition,
) -> Result<Option<PathBuf>, WorkflowError> {
    let workflow_dir = definition
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let logging_map = get_map(&definition.config, "logging");
    string_value(logging_map, "service_dir")
        .map(|raw| expand_path_value(&raw, &workflow_dir))
        .transpose()
}

fn default_service_logging_dir(workflow_dir: &Path) -> PathBuf {
    workflow_dir.join(".vik").join("service")
}
