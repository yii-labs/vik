use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use vik_core::WorkflowDefinition;

use crate::{ServiceConfig, WorkflowError};

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
    fn command_succeeds(&self, program: &str, args: &[&str]) -> bool;
}

#[derive(Debug, Default)]
pub struct SystemDiagnoseEnvironment;

impl DiagnoseEnvironment for SystemDiagnoseEnvironment {
    fn env_var_is_set(&self, name: &str) -> bool {
        env::var(name).is_ok_and(|value| !value.trim().is_empty())
    }

    fn command_succeeds(&self, program: &str, args: &[&str]) -> bool {
        let Some(program) = find_command(program) else {
            return false;
        };
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };

        let started_at = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if started_at.elapsed() < command_auth_timeout() => {
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                Err(_) => return false,
            }
        }
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

    fn diagnose_command_auth(
        &self,
        environment: &dyn DiagnoseEnvironment,
        diagnoses: &mut Diagnoses,
    ) {
        if environment.env_var_is_set("OPENAI_API_KEY") {
            diagnoses.push(Diagnosis::passed(
                "auth.codex",
                "OPENAI_API_KEY is set for Codex auth",
            ));
        } else if environment.command_succeeds("codex", &["login", "status"]) {
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

        if environment.env_var_is_set("GH_TOKEN") || environment.env_var_is_set("GITHUB_TOKEN") {
            diagnoses.push(Diagnosis::passed(
                "auth.github",
                "GitHub token environment is set",
            ));
        } else if environment.command_succeeds(
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
    }
}

fn workflow_error_message(err: WorkflowError) -> String {
    err.to_string()
}

fn command_auth_timeout() -> Duration {
    Duration::from_secs(5)
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
