use std::collections::BTreeSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use vik_core::WorkflowDefinition;

use crate::{ServiceConfig, WorkflowError, first_shell_token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosisSeverity {
    Passed,
    Warning,
    Error,
}

impl DiagnosisSeverity {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Passed => "ok",
            Self::Warning => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    pub name: String,
    pub severity: DiagnosisSeverity,
    pub message: String,
}

impl Diagnosis {
    pub fn passed(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            severity: DiagnosisSeverity::Passed,
            message: message.into(),
        }
    }

    pub fn warning(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            severity: DiagnosisSeverity::Warning,
            message: message.into(),
        }
    }

    pub fn error(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            severity: DiagnosisSeverity::Error,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnoses(pub Vec<Diagnosis>);

impl Diagnoses {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, diagnosis: Diagnosis) {
        self.0.push(diagnosis);
    }

    pub fn extend(&mut self, diagnoses: Diagnoses) {
        self.0.extend(diagnoses.0);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnosis> {
        self.0.iter()
    }

    pub fn has_errors(&self) -> bool {
        self.0
            .iter()
            .any(|diagnosis| diagnosis.severity == DiagnosisSeverity::Error)
    }

    pub fn count(&self, severity: DiagnosisSeverity) -> usize {
        self.0
            .iter()
            .filter(|diagnosis| diagnosis.severity == severity)
            .count()
    }
}

pub trait Diagnose {
    fn diagnose(&self, environment: &dyn DiagnoseEnvironment) -> Diagnoses;
}

pub trait DiagnoseEnvironment {
    fn env_var_is_set(&self, name: &str) -> bool;
    fn command_exists(&self, command: &str) -> bool;
    fn command_succeeds(&self, program: &str, args: &[&str]) -> bool;
}

#[derive(Debug, Default)]
pub struct SystemDiagnoseEnvironment;

impl DiagnoseEnvironment for SystemDiagnoseEnvironment {
    fn env_var_is_set(&self, name: &str) -> bool {
        env::var(name).is_ok_and(|value| !value.trim().is_empty())
    }

    fn command_exists(&self, command: &str) -> bool {
        find_command(command).is_some()
    }

    fn command_succeeds(&self, program: &str, args: &[&str]) -> bool {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

impl Diagnose for WorkflowDefinition {
    fn diagnose(&self, environment: &dyn DiagnoseEnvironment) -> Diagnoses {
        match ServiceConfig::from_definition(self) {
            Ok(config) => config.diagnose(environment),
            Err(err) => {
                let mut diagnoses = Diagnoses::new();
                diagnoses.push(Diagnosis::error(
                    "config.workflow",
                    format!("workflow config is invalid: {err}"),
                ));
                diagnoses
            }
        }
    }
}

impl Diagnose for ServiceConfig {
    fn diagnose(&self, environment: &dyn DiagnoseEnvironment) -> Diagnoses {
        let mut diagnoses = Diagnoses::new();
        self.diagnose_config(&mut diagnoses);
        self.diagnose_environment(environment, &mut diagnoses);
        diagnoses
    }
}

impl ServiceConfig {
    fn diagnose_config(&self, diagnoses: &mut Diagnoses) {
        match self.tracker.validate_without_api_key() {
            Ok(()) => diagnoses.push(Diagnosis::passed(
                "config.tracker",
                format!("{} tracker config shape is valid", self.tracker.kind_name()),
            )),
            Err(err) => diagnoses.push(Diagnosis::error(
                "config.tracker",
                workflow_error_message(err.into()),
            )),
        }

        if self.polling.interval_ms == 0 {
            diagnoses.push(Diagnosis::error(
                "config.polling",
                "polling.interval_ms must be positive",
            ));
        } else {
            diagnoses.push(Diagnosis::passed(
                "config.polling",
                "polling interval is positive",
            ));
        }

        match self.validate_codex_config() {
            Ok(()) => diagnoses.push(Diagnosis::passed(
                "config.codex",
                "codex runtime config shape is valid",
            )),
            Err(err) => diagnoses.push(Diagnosis::error(
                "config.codex",
                workflow_error_message(err),
            )),
        }
    }

    fn diagnose_environment(
        &self,
        environment: &dyn DiagnoseEnvironment,
        diagnoses: &mut Diagnoses,
    ) {
        self.diagnose_tracker_api_key(diagnoses);
        self.diagnose_required_commands(environment, diagnoses);
        self.diagnose_command_auth(environment, diagnoses);
    }

    fn diagnose_tracker_api_key(&self, diagnoses: &mut Diagnoses) {
        if self.tracker.has_api_key() {
            diagnoses.push(Diagnosis::passed(
                "env.tracker_api_key",
                format!("{} tracker API key is configured", self.tracker.kind_name()),
            ));
            return;
        }

        let env_names = self.tracker.api_key_env_names().join(" or ");
        diagnoses.push(Diagnosis::warning(
            "env.tracker_api_key",
            format!(
                "{env_names} is not set; vik start requires a {} API key",
                self.tracker.kind_name()
            ),
        ));
    }

    fn diagnose_required_commands(
        &self,
        environment: &dyn DiagnoseEnvironment,
        diagnoses: &mut Diagnoses,
    ) {
        for command in self.required_commands() {
            if environment.command_exists(&command) {
                diagnoses.push(Diagnosis::passed(
                    format!("command.{command}"),
                    format!("{command} command is available"),
                ));
            } else {
                diagnoses.push(Diagnosis::warning(
                    format!("command.{command}"),
                    format!("{command} command was not found on PATH"),
                ));
            }
        }
    }

    fn diagnose_command_auth(
        &self,
        environment: &dyn DiagnoseEnvironment,
        diagnoses: &mut Diagnoses,
    ) {
        let Some(codex_command) = self.codex.command_program() else {
            return;
        };

        if environment.env_var_is_set("OPENAI_API_KEY") {
            diagnoses.push(Diagnosis::passed(
                "auth.codex",
                "OPENAI_API_KEY is set for Codex auth",
            ));
        } else if is_command_named(&codex_command, "codex")
            && environment.command_exists(&codex_command)
        {
            if environment.command_succeeds(&codex_command, &["login", "status"]) {
                diagnoses.push(Diagnosis::passed(
                    "auth.codex",
                    "codex login status succeeds",
                ));
            } else {
                diagnoses.push(Diagnosis::warning(
                    "auth.codex",
                    "codex login status did not succeed; vik start requires Codex auth",
                ));
            }
        } else if is_command_named(&codex_command, "codex") {
            diagnoses.push(Diagnosis::warning(
                "auth.codex",
                "codex auth was not checked because the codex command was not found",
            ));
        } else {
            diagnoses.push(Diagnosis::warning(
                "auth.codex",
                format!("codex auth was not checked for configured command `{codex_command}`"),
            ));
        }

        if environment.env_var_is_set("GH_TOKEN") || environment.env_var_is_set("GITHUB_TOKEN") {
            diagnoses.push(Diagnosis::passed(
                "auth.github",
                "GitHub token environment is set",
            ));
        } else if environment.command_exists("gh") {
            if environment.command_succeeds(
                "gh",
                &["auth", "status", "--active", "--hostname", "github.com"],
            ) {
                diagnoses.push(Diagnosis::passed("auth.github", "gh auth status succeeds"));
            } else {
                diagnoses.push(Diagnosis::warning(
                    "auth.github",
                    "gh auth status did not succeed; agents need GitHub auth for PR workflows",
                ));
            }
        } else {
            diagnoses.push(Diagnosis::warning(
                "auth.github",
                "gh auth was not checked because the gh command was not found and no GitHub token is set",
            ));
        }
    }

    fn required_commands(&self) -> BTreeSet<String> {
        let mut commands = BTreeSet::from(["gh".to_string(), "git".to_string()]);
        if let Some(command) = self.codex.command_program() {
            commands.insert(command);
        }
        for hook in [
            self.hooks.after_create.as_deref(),
            self.hooks.before_run.as_deref(),
            self.hooks.after_run.as_deref(),
            self.hooks.before_remove.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            commands.extend(hook_command_names(hook));
        }
        commands
    }
}

fn hook_command_names(hook: &str) -> impl Iterator<Item = String> + '_ {
    hook.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(first_shell_token)
        .filter(|command| !is_shell_syntax(command))
}

fn workflow_error_message(err: WorkflowError) -> String {
    err.to_string()
}

fn is_shell_syntax(command: &str) -> bool {
    matches!(
        command,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "for"
            | "while"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "{"
            | "}"
            | "export"
            | "set"
            | "cd"
            | ":"
    )
}

fn is_command_named(command: &str, name: &str) -> bool {
    let file_name = Path::new(command)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(command);
    file_name == name || file_name == format!("{name}.exe")
}

fn find_command(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }

    if command.contains('/') || command.contains('\\') {
        let path = PathBuf::from(command);
        return command_candidates(&path)
            .into_iter()
            .find(|candidate| is_executable_file(candidate));
    }

    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .flat_map(|path| command_candidates(&path.join(command)))
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(windows)]
fn command_candidates(path: &Path) -> Vec<PathBuf> {
    if path.extension().is_some() {
        return vec![path.to_path_buf()];
    }
    let extensions =
        env::var_os("PATHEXT").unwrap_or_else(|| std::ffi::OsString::from(".EXE;.BAT;.CMD;.COM"));
    env::split_paths(&extensions)
        .map(|extension| path.with_extension(extension.to_string_lossy().trim_start_matches('.')))
        .collect()
}

#[cfg(not(windows))]
fn command_candidates(path: &Path) -> Vec<PathBuf> {
    vec![path.to_path_buf()]
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}
