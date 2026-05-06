use std::env;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use inquire::Confirm;
use serde::{Deserialize, Serialize};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, Layer, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};
use vik_agent::LocalAgentWorker;
use vik_http::{HttpState, serve};
use vik_orchestrator::Orchestrator;
use vik_tracker::TrackerClient;
use vik_workflow::{WorkflowReloader, load_effective_workflow, select_workflow_path};

use crate::env::load_dotenv_from_dir;
use crate::service::{RunArgs, StartArgs};

pub struct ServiceManager {
    cwd: PathBuf,
    workflow_path: PathBuf,
    service_dir: PathBuf,
    state_path: PathBuf,
    log_dir: PathBuf,
    session_dir: PathBuf,
}

impl ServiceManager {
    pub fn new(workflow: Option<PathBuf>) -> Result<Self, Box<dyn Error>> {
        let selected = select_workflow_path(workflow);
        let workflow_path = if selected.is_absolute() {
            selected
        } else {
            env::current_dir()?.join(selected)
        };
        let workflow_path = normalize_path(workflow_path);

        if let Some(workflow_dir) = workflow_path.parent() {
            load_dotenv_from_dir(workflow_dir)?;
        }
        let loaded = load_effective_workflow(Some(workflow_path.clone()))?;
        let cwd = normalize_path(loaded.config.workspace.root);
        let service_dir = cwd.join("service");
        let log_dir = normalize_path(loaded.config.logging.dir);
        let session_dir = service_dir.join("sessions");

        let stem = workflow_path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(|value| {
                value
                    .chars()
                    .map(|ch| {
                        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                            ch
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>()
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "workflow".to_string());
        let mut hash = 0xcbf29ce484222325;
        for byte in workflow_path.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let name = format!("{stem}-{hash:016x}");

        Ok(Self {
            cwd,
            workflow_path,
            service_dir: service_dir.clone(),
            state_path: service_dir.join(format!("{name}.json")),
            log_dir,
            session_dir,
        })
    }

    pub async fn start(&self, args: StartArgs) -> Result<(), Box<dyn Error>> {
        if args.detached {
            self.start_detached(args.run_args.with_default_service_port())
        } else {
            self.start_foreground(args.run_args).await
        }
    }

    pub fn stop(&self) -> Result<(), Box<dyn Error>> {
        let _ = self.stop_inner(false)?;
        Ok(())
    }

    pub async fn restart(&self, args: RunArgs) -> Result<(), Box<dyn Error>> {
        self.validate_for_dispatch()?;
        let state = self.read_state()?;
        if state.as_ref().map(|state| self.classify_state(state)) == Some(RuntimeStatus::Running) {
            let _ = self.stop_inner(false)?;
            let pid = self.spawn_detached(args, state.as_ref())?;
            self.print_service_restarted(pid);
            return Ok(());
        }

        if !self.confirm_start_when_not_running()? {
            println!("service start skipped");
            return Ok(());
        }

        if state.is_some() {
            let _ = self.stop_inner(false)?;
        }
        let pid = self.spawn_detached(args, state.as_ref())?;
        self.print_service_started(pid);
        Ok(())
    }

    pub fn status(&self) -> Result<(), Box<dyn Error>> {
        let Some(state) = self.read_state()? else {
            println!("not installed: {}", self.state_path.display());
            return Ok(());
        };

        match self.classify_state(&state) {
            RuntimeStatus::Running => println!(
                "running: pid={} workflow={} log={}",
                state.pid.unwrap_or_default(),
                state.workflow_path.display(),
                state.log_dir.display()
            ),
            RuntimeStatus::Stopped => println!(
                "stopped: workflow={} log={}",
                state.workflow_path.display(),
                state.log_dir.display()
            ),
            RuntimeStatus::Stale => println!(
                "stale: pid={} workflow={} log={}",
                state.pid.unwrap_or_default(),
                state.workflow_path.display(),
                state.log_dir.display()
            ),
        }
        Ok(())
    }

    pub fn uninstall(&self) -> Result<(), Box<dyn Error>> {
        if self.stop_inner(true)? {
            println!("service uninstalled: {}", self.state_path.display());
        }
        Ok(())
    }

    fn start_detached(&self, args: RunArgs) -> Result<(), Box<dyn Error>> {
        self.validate_for_dispatch()?;
        let previous_state = self.read_state()?;
        if let Some(state) = &previous_state
            && self.classify_state(state) == RuntimeStatus::Running
        {
            println!(
                "service already running: pid={} log={}",
                state.pid.unwrap_or_default(),
                state.log_dir.display()
            );
            return Ok(());
        }

        let pid = self.spawn_detached(args, previous_state.as_ref())?;
        self.print_service_started(pid);
        Ok(())
    }

    async fn start_foreground(&self, args: RunArgs) -> Result<(), Box<dyn Error>> {
        self.load_dotenv_from_cwd()?;
        let reloader = WorkflowReloader::start(Some(self.workflow_path.clone()))?;
        self.run_foreground(reloader, args.host, args.port).await
    }

    fn spawn_detached(
        &self,
        args: RunArgs,
        previous_state: Option<&ServiceState>,
    ) -> Result<u32, Box<dyn Error>> {
        let cwd = self.effective_cwd(previous_state);
        fs::create_dir_all(&self.service_dir)?;
        fs::create_dir_all(&self.log_dir)?;
        let executable = env::current_exe()?;
        let mut command = Command::new(&executable);
        for arg in self.daemon_args(args) {
            command.arg(arg);
        }
        command.current_dir(&cwd);
        self.detach_command(&mut command);

        let err_log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.service_stderr_log_path(&self.log_dir))?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(err_log));

        let child = command.spawn()?;
        let pid = child.id();
        let state = ServiceState {
            version: 1,
            workflow_path: self.workflow_path.clone(),
            cwd,
            pid: Some(pid),
            status: StoredStatus::Running,
            started_at: Some(self.now()),
            stopped_at: None,
            log_dir: self.log_dir.clone(),
            session_dir: self.session_dir.clone(),
            port: args.port,
            command: self.display_command(&executable, args),
        };
        self.write_state(&state)?;
        Ok(pid)
    }

    fn stop_inner(&self, remove_state: bool) -> Result<bool, Box<dyn Error>> {
        let Some(mut state) = self.read_state()? else {
            println!("service not installed: {}", self.state_path.display());
            return Ok(false);
        };

        match self.classify_state(&state) {
            RuntimeStatus::Running => {
                let pid = state.pid.unwrap_or_default();
                self.terminate_pid(pid)?;
                state.status = StoredStatus::Stopped;
                state.stopped_at = Some(self.now());
                println!("service stopped: pid={pid}");
            }
            RuntimeStatus::Stopped => {
                println!("service already stopped: {}", self.state_path.display());
            }
            RuntimeStatus::Stale => {
                let pid = state.pid.unwrap_or_default();
                if pid != 0 && self.stale_service_group_cleanup_allowed(pid) {
                    self.terminate_stale_service_processes(pid)?;
                }
                state.status = StoredStatus::Stopped;
                state.stopped_at = Some(self.now());
                println!("service stale: pid={pid} is not running");
            }
        }

        if remove_state {
            self.remove_state_file()?;
        } else {
            self.write_state(&state)?;
        }
        Ok(true)
    }

    fn validate_for_dispatch(&self) -> Result<(), Box<dyn Error>> {
        self.load_dotenv_from_cwd()?;
        let loaded = load_effective_workflow(Some(self.workflow_path.clone()))?;
        loaded.config.validate_for_dispatch()?;
        Ok(())
    }

    async fn run_foreground(
        &self,
        reloader: WorkflowReloader,
        host: IpAddr,
        port: Option<u16>,
    ) -> Result<(), Box<dyn Error>> {
        let loaded = reloader.current().clone();
        loaded.config.validate_for_dispatch()?;

        let _log_guard = self.init_logging(&self.log_dir)?;
        tracing::info!(log_dir=%self.log_dir.display(), "logging outcome=started");

        let tracker = Arc::new(TrackerClient::from_config(&loaded.config.tracker)?);
        let worker = Arc::new(LocalAgentWorker::new(Arc::clone(&tracker)));
        let orchestrator = Arc::new(Orchestrator::new(Arc::clone(&tracker), worker, reloader));

        let port = port.or(loaded.config.server.as_ref().map(|server| server.port));
        if let Some(port) = port {
            let orch_for_state = Arc::clone(&orchestrator);
            let orch_for_issue = Arc::clone(&orchestrator);
            let addr = SocketAddr::new(host, port);
            let bound = serve(
                addr,
                HttpState {
                    snapshot: Arc::new(move || {
                        let orch = Arc::clone(&orch_for_state);
                        Box::pin(async move { orch.snapshot().await })
                    }),
                    issue: Arc::new(move |identifier| {
                        let orch = Arc::clone(&orch_for_issue);
                        Box::pin(async move { orch.issue_debug(&identifier).await })
                    }),
                    refresh_tx: orchestrator.refresh_sender(),
                },
            )
            .await?;
            tracing::info!(addr=%bound, "http_server outcome=started");
        }

        orchestrator.run_forever().await?;
        Ok(())
    }

    fn init_logging(&self, log_dir: &Path) -> Result<(WorkerGuard, WorkerGuard), Box<dyn Error>> {
        fs::create_dir_all(log_dir)?;
        let file_appender = tracing_appender::rolling::daily(log_dir, "vik.log");
        let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
        let error_file_appender = tracing_appender::rolling::daily(log_dir, "vik-error.log");
        let (error_file_writer, error_guard) = tracing_appender::non_blocking(error_file_appender);
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        let stdout_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(false)
            .with_span_list(false);
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(file_writer)
            .json()
            .with_current_span(false)
            .with_span_list(false);
        let error_file_layer = tracing_subscriber::fmt::layer()
            .with_writer(error_file_writer)
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .with_filter(LevelFilter::ERROR);

        tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .with(file_layer)
            .with(error_file_layer)
            .init();
        Ok((guard, error_guard))
    }

    fn load_dotenv_from_cwd(&self) -> Result<(), Box<dyn Error>> {
        load_dotenv_from_dir(&self.cwd)
    }

    fn read_state(&self) -> Result<Option<ServiceState>, Box<dyn Error>> {
        match fs::read_to_string(&self.state_path) {
            Ok(body) => Ok(Some(serde_json::from_str(&body)?)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn write_state(&self, state: &ServiceState) -> Result<(), Box<dyn Error>> {
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &self.state_path,
            format!("{}\n", serde_json::to_string_pretty(state)?),
        )?;
        Ok(())
    }

    fn remove_state_file(&self) -> Result<(), Box<dyn Error>> {
        match fs::remove_file(&self.state_path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    fn effective_cwd(&self, previous_state: Option<&ServiceState>) -> PathBuf {
        previous_state
            .map(|state| state.cwd.clone())
            .unwrap_or_else(|| self.cwd.clone())
    }

    fn daemon_args(&self, args: RunArgs) -> Vec<String> {
        let port = args.port.unwrap_or(super::DEFAULT_SERVICE_PORT);
        vec![
            "start".to_string(),
            self.workflow_path.display().to_string(),
            "--port".to_string(),
            port.to_string(),
            "--host".to_string(),
            args.host.to_string(),
        ]
    }

    fn display_command(&self, executable: &Path, args: RunArgs) -> Vec<String> {
        let mut command = vec![executable.display().to_string()];
        command.extend(self.daemon_args(args));
        command
    }

    fn print_service_started(&self, pid: u32) {
        println!(
            "service started: pid={} state={} log={}",
            pid,
            self.state_path.display(),
            self.log_dir.display()
        );
    }

    fn print_service_restarted(&self, pid: u32) {
        println!(
            "service restarted: pid={} state={} log={}",
            pid,
            self.state_path.display(),
            self.log_dir.display()
        );
    }

    fn service_stderr_log_path(&self, log_dir: &Path) -> PathBuf {
        log_dir.join("vik-service.log")
    }

    fn classify_state(&self, state: &ServiceState) -> RuntimeStatus {
        match (state.status, state.pid) {
            (StoredStatus::Running, Some(pid)) if self.process_matches_state(pid, state) => {
                RuntimeStatus::Running
            }
            (StoredStatus::Running, Some(_)) => RuntimeStatus::Stale,
            _ => RuntimeStatus::Stopped,
        }
    }

    fn stale_service_group_cleanup_allowed(&self, pid: u32) -> bool {
        !self.process_alive(pid)
    }

    fn confirm_start_when_not_running(&self) -> Result<bool, Box<dyn Error>> {
        Ok(
            Confirm::new("service is not running, do you want to start now?")
                .with_default(true)
                .prompt()?,
        )
    }

    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    #[cfg(unix)]
    fn detach_command(&self, command: &mut Command) {
        use std::os::unix::process::CommandExt;

        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }

    #[cfg(windows)]
    fn detach_command(&self, command: &mut Command) {
        use std::os::windows::process::CommandExt;

        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    #[cfg(unix)]
    fn process_alive(&self, pid: u32) -> bool {
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if result == 0 {
            return true;
        }
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(unix)]
    fn process_group_alive(&self, pid: u32) -> bool {
        let process_group = -(pid as libc::pid_t);
        let result = unsafe { libc::kill(process_group, 0) };
        if result == 0 {
            return true;
        }
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(unix)]
    fn process_matches_state(&self, pid: u32, state: &ServiceState) -> bool {
        if !self.process_alive(pid) {
            return false;
        }

        let Ok(output) = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
        else {
            return true;
        };
        if !output.status.success() {
            return true;
        }
        let Ok(command) = String::from_utf8(output.stdout) else {
            return true;
        };
        self.command_mentions_workflow(&command, &state.workflow_path)
    }

    #[cfg(windows)]
    fn process_alive(&self, pid: u32) -> bool {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}")])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }

    #[cfg(windows)]
    fn process_matches_state(&self, pid: u32, state: &ServiceState) -> bool {
        if !self.process_alive(pid) {
            return false;
        }
        let filter = format!("ProcessId = {pid}");
        let Ok(output) = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-CimInstance Win32_Process -Filter '{filter}').CommandLine"),
            ])
            .output()
        else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        self.command_mentions_workflow(
            String::from_utf8_lossy(&output.stdout).as_ref(),
            &state.workflow_path,
        )
    }

    fn command_mentions_workflow(&self, command: &str, workflow_path: &Path) -> bool {
        command.contains(workflow_path.to_string_lossy().as_ref())
    }

    #[cfg(unix)]
    fn terminate_pid(&self, pid: u32) -> io::Result<()> {
        if !self.process_group_alive(pid) && !self.process_alive(pid) {
            return Ok(());
        }
        self.signal_service_process_group(pid, libc::SIGTERM)?;
        if self.wait_until_process_group_dead(pid, Duration::from_secs(5)) {
            return Ok(());
        }
        self.signal_service_process_group(pid, libc::SIGKILL)?;
        let _ = self.wait_until_process_group_dead(pid, Duration::from_secs(2));
        Ok(())
    }

    #[cfg(unix)]
    fn terminate_stale_service_processes(&self, pid: u32) -> io::Result<()> {
        if !self.process_group_alive(pid) {
            return Ok(());
        }
        self.signal_service_process_group(pid, libc::SIGTERM)?;
        if self.wait_until_process_group_dead(pid, Duration::from_secs(5)) {
            return Ok(());
        }
        self.signal_service_process_group(pid, libc::SIGKILL)?;
        let _ = self.wait_until_process_group_dead(pid, Duration::from_secs(2));
        Ok(())
    }

    #[cfg(unix)]
    fn signal_service_process_group(&self, pid: u32, signal: libc::c_int) -> io::Result<()> {
        // Services call setsid() on spawn, so service pid is also process group id.
        let process_group = -(pid as libc::pid_t);
        let result = unsafe { libc::kill(process_group, signal) };
        if result == 0 {
            return Ok(());
        }
        let group_err = io::Error::last_os_error();
        if group_err.raw_os_error() != Some(libc::ESRCH) || !self.process_alive(pid) {
            return Err(group_err);
        }

        let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(windows)]
    fn terminate_pid(&self, pid: u32) -> io::Result<()> {
        if !self.process_alive(pid) {
            return Ok(());
        }
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("taskkill failed for pid {pid}")))
        }
    }

    #[cfg(windows)]
    fn terminate_stale_service_processes(&self, _pid: u32) -> io::Result<()> {
        Ok(())
    }

    #[cfg(unix)]
    fn wait_until_process_group_dead(&self, pid: u32, timeout: Duration) -> bool {
        let start = SystemTime::now();
        while self.process_group_alive(pid) {
            if start.elapsed().unwrap_or_default() >= timeout {
                return false;
            }
            thread::sleep(Duration::from_millis(100));
        }
        true
    }
}

impl Default for RunArgs {
    fn default() -> Self {
        Self {
            port: Some(super::DEFAULT_SERVICE_PORT),
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        }
    }
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceState {
    version: u32,
    cwd: PathBuf,
    workflow_path: PathBuf,
    pid: Option<u32>,
    status: StoredStatus,
    #[serde(alias = "started_at_unix")]
    started_at: Option<u64>,
    #[serde(alias = "stopped_at_unix")]
    stopped_at: Option<u64>,
    #[serde(alias = "log_path")]
    log_dir: PathBuf,
    #[serde(default)]
    session_dir: PathBuf,
    port: Option<u16>,
    command: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredStatus {
    Running,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeStatus {
    Running,
    Stopped,
    Stale,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_is_ascii_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("Vik Workflow.md");
        let manager = test_manager(&workflow_path, "work").unwrap();
        let second = test_manager(&workflow_path, "work").unwrap();
        let name = manager.state_path.file_stem().unwrap().to_str().unwrap();

        assert_eq!(manager.state_path, second.state_path);
        assert!(name.starts_with("Vik_Workflow-"));
        assert!(
            name.chars()
                .all(|ch| { ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' })
        );
    }

    #[test]
    fn daemon_args_include_workflow_host_and_port() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");
        let manager = test_manager(&workflow_path, "work").unwrap();

        assert_eq!(
            manager.daemon_args(RunArgs::default()),
            vec![
                "start",
                manager.workflow_path.to_str().unwrap(),
                "--port",
                "7788",
                "--host",
                "127.0.0.1"
            ]
        );
    }

    #[test]
    fn effective_service_cwd_reuses_stored_cwd() {
        let manager = temp_manager().unwrap();
        let state = service_state_with_cwd(PathBuf::from("/tmp/installed-cwd"), None);

        assert_eq!(
            manager.effective_cwd(Some(&state)),
            PathBuf::from("/tmp/installed-cwd")
        );
    }

    #[test]
    fn manager_cwd_uses_resolved_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");

        let manager = test_manager(&workflow_path, "work").unwrap();

        assert_eq!(manager.cwd, dir.path().join("work"));
        assert_eq!(manager.service_dir, manager.cwd.join("service"));
        assert_eq!(manager.log_dir, manager.cwd.join("logs"));
    }

    #[test]
    fn manager_cwd_uses_workspace_root_from_dotenv() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");
        let key = unique_env_key("WORKSPACE_ROOT");
        fs::write(dir.path().join(".env"), format!("{key}=env-workspace\n")).unwrap();
        write_workflow(&workflow_path, &format!("${key}"));

        let manager = ServiceManager::new(Some(workflow_path)).unwrap();

        assert_eq!(manager.cwd, dir.path().join("env-workspace"));
    }

    #[test]
    fn manager_state_path_uses_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");
        let first = test_manager(&workflow_path, "work-a").unwrap();
        let second = test_manager(&workflow_path, "work-b").unwrap();

        assert_ne!(first.state_path, second.state_path);
        assert_eq!(first.service_dir, dir.path().join("work-a").join("service"));
        assert_eq!(
            second.service_dir,
            dir.path().join("work-b").join("service")
        );
    }

    #[test]
    fn manager_state_path_normalizes_equivalent_workflow_paths() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");
        let dotted_workflow_path = dir.path().join(".").join("WORKFLOW.md");
        write_workflow(&workflow_path, "work");

        let first = ServiceManager::new(Some(workflow_path.clone())).unwrap();
        let second = ServiceManager::new(Some(dotted_workflow_path)).unwrap();

        assert_eq!(first.workflow_path, workflow_path);
        assert_eq!(first.state_path, second.state_path);
    }

    #[cfg(unix)]
    #[test]
    fn manager_preserves_symlinked_workflow_path() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("real");
        let link_dir = dir.path().join("link");
        fs::create_dir_all(&real_dir).unwrap();
        fs::create_dir_all(&link_dir).unwrap();
        let real_workflow = real_dir.join("WORKFLOW.md");
        let link_workflow = link_dir.join("WORKFLOW.md");
        write_workflow(&real_workflow, "work");
        std::os::unix::fs::symlink(&real_workflow, &link_workflow).unwrap();

        let manager = ServiceManager::new(Some(link_workflow.clone())).unwrap();

        assert_eq!(manager.workflow_path, link_workflow);
        assert_ne!(
            fs::canonicalize(&manager.workflow_path).unwrap(),
            manager.workflow_path
        );
    }

    #[test]
    fn command_match_requires_workflow_path() {
        let manager = temp_manager().unwrap();
        let workflow_path = PathBuf::from("/tmp/vik/WORKFLOW.md");

        assert!(
            manager
                .command_mentions_workflow("vik /tmp/vik/WORKFLOW.md --port 3000", &workflow_path)
        );
        assert!(!manager.command_mentions_workflow("vik /tmp/other/WORKFLOW.md", &workflow_path));
    }

    #[test]
    fn load_dotenv_from_cwd_walks_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested").join("service");
        fs::create_dir_all(&nested).unwrap();
        let workflow_path = nested.join("WORKFLOW.md");
        let manager = test_manager(&workflow_path, "work").unwrap();
        let key = unique_env_key("SERVICE");
        fs::write(dir.path().join(".env"), format!("{key}=from_service_dir\n")).unwrap();

        manager.load_dotenv_from_cwd().unwrap();

        assert_eq!(env::var(key).unwrap(), "from_service_dir");
    }

    #[test]
    fn manager_reports_missing_workflow_file() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("missing.md");

        let err = match ServiceManager::new(Some(workflow_path)) {
            Ok(_) => panic!("expected missing workflow file error"),
            Err(err) => err.to_string(),
        };

        assert!(err.contains("missing_workflow_file"));
    }

    #[test]
    fn stopped_state_classifies_as_stopped_without_pid_probe() {
        let manager = temp_manager().unwrap();
        let state = ServiceState {
            version: 1,
            workflow_path: PathBuf::from("/tmp/vik/WORKFLOW.md"),
            cwd: PathBuf::from("/tmp/vik"),
            pid: Some(999_999),
            status: StoredStatus::Stopped,
            started_at: Some(1),
            stopped_at: Some(2),
            log_dir: PathBuf::from("/tmp/vik/service.log"),
            session_dir: PathBuf::from("/tmp/vik/sessions"),
            port: None,
            command: vec![],
        };

        assert_eq!(manager.classify_state(&state), RuntimeStatus::Stopped);
    }

    #[test]
    fn service_state_reads_legacy_log_schema() {
        let body = r#"{
            "version": 1,
            "workflow_path": "/tmp/vik/WORKFLOW.md",
            "cwd": "/tmp/vik",
            "pid": null,
            "status": "stopped",
            "started_at_unix": 1,
            "stopped_at_unix": 2,
            "log_path": "/tmp/vik/service.log",
            "port": null,
            "command": []
        }"#;

        let state: ServiceState = serde_json::from_str(body).unwrap();

        assert_eq!(state.started_at, Some(1));
        assert_eq!(state.stopped_at, Some(2));
        assert_eq!(state.log_dir, PathBuf::from("/tmp/vik/service.log"));
        assert!(state.session_dir.as_os_str().is_empty());
    }

    #[test]
    fn stale_group_cleanup_skips_live_pid() {
        let manager = temp_manager().unwrap();

        assert!(!manager.stale_service_group_cleanup_allowed(std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn signal_service_process_group_stops_descendants() {
        let manager = temp_manager().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let child_pid_path = dir.path().join("child.pid");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 60 & echo $! > \"$CHILD_PID_FILE\"; wait")
            .env("CHILD_PID_FILE", &child_pid_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        manager.detach_command(&mut command);

        let mut child = command.spawn().unwrap();
        let service_pid = child.id();
        let child_pid = read_child_pid(&child_pid_path);

        manager
            .signal_service_process_group(service_pid, libc::SIGTERM)
            .unwrap();
        let _ = child.wait();

        assert!(wait_for_dead(&manager, child_pid, Duration::from_secs(2)));
    }

    #[cfg(unix)]
    #[test]
    fn terminate_stale_service_processes_stops_group_without_leader() {
        let manager = temp_manager().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let child_pid_path = dir.path().join("child.pid");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 60 & echo $! > \"$CHILD_PID_FILE\"")
            .env("CHILD_PID_FILE", &child_pid_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        manager.detach_command(&mut command);

        let mut child = command.spawn().unwrap();
        let service_pid = child.id();
        let child_pid = read_child_pid(&child_pid_path);
        let _ = child.wait();

        assert!(!manager.process_alive(service_pid));
        assert!(manager.process_alive(child_pid));

        manager
            .terminate_stale_service_processes(service_pid)
            .unwrap();

        assert!(wait_for_dead(&manager, child_pid, Duration::from_secs(2)));
    }

    #[cfg(unix)]
    #[test]
    fn terminate_pid_force_kills_descendants_after_leader_exit() {
        let manager = temp_manager().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let child_pid_path = dir.path().join("child.pid");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("sh -c 'trap \"\" TERM; while true; do sleep 1; done' & echo $! > \"$CHILD_PID_FILE\"; wait")
            .env("CHILD_PID_FILE", &child_pid_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        manager.detach_command(&mut command);

        let mut child = command.spawn().unwrap();
        let service_pid = child.id();
        let child_pid = read_child_pid(&child_pid_path);

        manager.terminate_pid(service_pid).unwrap();
        let _ = child.wait();

        assert!(wait_for_dead(&manager, child_pid, Duration::from_secs(2)));
    }

    fn temp_manager() -> Result<ServiceManager, Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let workflow_path = dir.keep().join("WORKFLOW.md");
        test_manager(&workflow_path, "work")
    }

    fn test_manager(path: &Path, workspace_root: &str) -> Result<ServiceManager, Box<dyn Error>> {
        write_workflow(path, workspace_root);
        ServiceManager::new(Some(path.to_path_buf()))
    }

    fn service_state_with_cwd(cwd: PathBuf, port: Option<u16>) -> ServiceState {
        ServiceState {
            version: 1,
            workflow_path: PathBuf::from("/tmp/vik/WORKFLOW.md"),
            cwd,
            pid: None,
            status: StoredStatus::Stopped,
            started_at: Some(1),
            stopped_at: Some(2),
            log_dir: PathBuf::from("/tmp/vik/service.log"),
            session_dir: PathBuf::from("/tmp/vik/sessions"),
            port,
            command: vec![],
        }
    }

    fn write_workflow(path: &Path, workspace_root: &str) {
        fs::write(
            path,
            format!(
                "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\nworkspace:\n  root: {workspace_root}\n---\nBody"
            ),
        )
        .unwrap();
    }

    fn unique_env_key(suffix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("VIK_TEST_SERVICE_DOTENV_{nanos}_{suffix}")
    }

    #[cfg(unix)]
    fn read_child_pid(path: &Path) -> u32 {
        let start = SystemTime::now();
        loop {
            if let Ok(body) = fs::read_to_string(path)
                && let Ok(pid) = body.trim().parse()
            {
                return pid;
            }
            assert!(
                start.elapsed().unwrap_or_default() < Duration::from_secs(2),
                "child pid file was not written"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    fn wait_for_dead(manager: &ServiceManager, pid: u32, timeout: Duration) -> bool {
        let start = SystemTime::now();
        while manager.process_alive(pid) {
            if start.elapsed().unwrap_or_default() >= timeout {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
        true
    }
}
