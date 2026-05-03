use std::collections::VecDeque;
use std::env;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args as ClapArgs, Subcommand};
use serde::{Deserialize, Serialize};
use vik_workflow::load_effective_workflow;

#[derive(Debug, ClapArgs)]
pub(crate) struct ServiceArgs {
    #[command(subcommand)]
    command: ServiceCommand,
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Install service state and start Vik in the background.
    Install(RunArgs),
    /// Remove service state and stop Vik if it is running.
    Uninstall(WorkflowArg),
    /// Print current service status.
    Status(WorkflowArg),
    /// Print recent service logs.
    Logs(LogsArgs),
    /// Start Vik in the background.
    Start(RunArgs),
    /// Stop a running Vik service.
    Stop(WorkflowArg),
    /// Stop then start Vik in the background.
    Restart(RunArgs),
}

#[derive(Debug, Clone, ClapArgs)]
struct RunArgs {
    /// Path to WORKFLOW.md. Defaults to ./WORKFLOW.md.
    workflow: Option<PathBuf>,

    /// Enable HTTP status server. Overrides server.port from WORKFLOW.md.
    #[arg(long)]
    port: Option<u16>,
}

#[derive(Debug, Clone, ClapArgs)]
struct WorkflowArg {
    /// Path to WORKFLOW.md. Defaults to ./WORKFLOW.md.
    workflow: Option<PathBuf>,
}

#[derive(Debug, Clone, ClapArgs)]
struct LogsArgs {
    /// Path to WORKFLOW.md. Defaults to ./WORKFLOW.md.
    workflow: Option<PathBuf>,

    /// Number of recent lines to print.
    #[arg(long, default_value_t = 100)]
    lines: usize,

    /// Continue printing appended log output.
    #[arg(long)]
    follow: bool,
}

#[derive(Debug)]
struct ServiceTarget {
    workflow_path: PathBuf,
    service_dir: PathBuf,
    state_path: PathBuf,
    log_path: PathBuf,
    cwd: PathBuf,
    port: Option<u16>,
}

impl ServiceTarget {
    fn load(
        workflow: Option<PathBuf>,
        port: Option<u16>,
        require_dispatch_config: bool,
    ) -> Result<Self, Box<dyn Error>> {
        let explicit_workflow = workflow.is_some();
        let workflow_path = if require_dispatch_config {
            let loaded = load_effective_workflow(workflow)?;
            loaded.config.validate_for_dispatch()?;
            fs::canonicalize(&loaded.definition.path)?
        } else {
            resolve_workflow_path(workflow)?
        };
        let cwd = service_cwd_for_workflow(&workflow_path, explicit_workflow)?;
        let service_dir = service_dir_for_workflow(&workflow_path);
        let name = service_name(&workflow_path);
        Ok(Self {
            workflow_path,
            service_dir: service_dir.clone(),
            state_path: service_dir.join(format!("{name}.json")),
            log_path: service_dir.join(format!("{name}.log")),
            cwd,
            port,
        })
    }

    fn load_for_dispatch(
        workflow: Option<PathBuf>,
        port: Option<u16>,
    ) -> Result<Self, Box<dyn Error>> {
        let management_target = Self::load(workflow, port, false)?;
        let previous_state = read_state(&management_target.state_path)?;
        let dotenv_dir = management_target.effective_cwd(previous_state.as_ref());
        load_dotenv_from_dir(&dotenv_dir)?;
        Self::load(Some(management_target.workflow_path), port, true)
    }

    fn start(&self, verb: &str) -> Result<(), Box<dyn Error>> {
        let previous_state = read_state(&self.state_path)?;
        if let Some(state) = &previous_state
            && classify_state(state) == RuntimeStatus::Running
        {
            println!(
                "service already running: pid={} log={}",
                state.pid.unwrap_or_default(),
                state.log_path.display()
            );
            return Ok(());
        }

        let port = self.effective_port(previous_state.as_ref());
        let cwd = self.effective_cwd(previous_state.as_ref());
        fs::create_dir_all(&self.service_dir)?;
        let executable = env::current_exe()?;
        let mut command = Command::new(&executable);
        for arg in self.daemon_args(port) {
            command.arg(arg);
        }
        command.current_dir(&cwd);
        detach_command(&mut command);

        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        let err_log = log.try_clone()?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err_log));

        let child = command.spawn()?;
        let pid = child.id();
        let state = ServiceState {
            version: 1,
            workflow_path: self.workflow_path.clone(),
            cwd,
            pid: Some(pid),
            status: StoredStatus::Running,
            started_at_unix: Some(now_unix()),
            stopped_at_unix: None,
            log_path: self.log_path.clone(),
            port,
            command: self.display_command(&executable, port),
        };
        write_state(&self.state_path, &state)?;
        println!(
            "service {verb}: pid={} state={} log={}",
            pid,
            self.state_path.display(),
            self.log_path.display()
        );
        Ok(())
    }

    fn stop(&self, remove_state: bool) -> Result<bool, Box<dyn Error>> {
        let Some(mut state) = read_state(&self.state_path)? else {
            println!("service not installed: {}", self.state_path.display());
            return Ok(false);
        };

        match classify_state(&state) {
            RuntimeStatus::Running => {
                let pid = state.pid.unwrap_or_default();
                terminate_pid(pid)?;
                state.status = StoredStatus::Stopped;
                state.stopped_at_unix = Some(now_unix());
                println!("service stopped: pid={pid}");
            }
            RuntimeStatus::Stopped => {
                println!("service already stopped: {}", self.state_path.display());
            }
            RuntimeStatus::Stale => {
                let pid = state.pid.unwrap_or_default();
                if pid != 0 && stale_service_group_cleanup_allowed(pid) {
                    terminate_stale_service_processes(pid)?;
                }
                state.status = StoredStatus::Stopped;
                state.stopped_at_unix = Some(now_unix());
                println!("service stale: pid={pid} is not running");
            }
        }

        if remove_state {
            remove_state_file(&self.state_path)?;
        } else {
            write_state(&self.state_path, &state)?;
        }
        Ok(true)
    }

    fn uninstall(&self) -> Result<(), Box<dyn Error>> {
        if self.stop(true)? {
            println!("service uninstalled: {}", self.state_path.display());
        }
        Ok(())
    }

    fn print_status(&self) -> Result<(), Box<dyn Error>> {
        let Some(state) = read_state(&self.state_path)? else {
            println!("not installed: {}", self.state_path.display());
            return Ok(());
        };

        match classify_state(&state) {
            RuntimeStatus::Running => println!(
                "running: pid={} workflow={} log={}",
                state.pid.unwrap_or_default(),
                state.workflow_path.display(),
                state.log_path.display()
            ),
            RuntimeStatus::Stopped => println!(
                "stopped: workflow={} log={}",
                state.workflow_path.display(),
                state.log_path.display()
            ),
            RuntimeStatus::Stale => println!(
                "stale: pid={} workflow={} log={}",
                state.pid.unwrap_or_default(),
                state.workflow_path.display(),
                state.log_path.display()
            ),
        }
        Ok(())
    }

    fn print_logs(&self, lines: usize, follow: bool) -> Result<(), Box<dyn Error>> {
        let path = read_state(&self.state_path)?
            .map(|state| state.log_path)
            .unwrap_or_else(|| self.log_path.clone());
        if !path.exists() {
            println!("service logs not found: {}", path.display());
            return Ok(());
        }

        print_recent_lines(&path, lines)?;
        if follow {
            follow_log(&path)?;
        }
        Ok(())
    }

    fn effective_port(&self, previous_state: Option<&ServiceState>) -> Option<u16> {
        self.port
            .or_else(|| previous_state.and_then(|state| state.port))
    }

    fn effective_cwd(&self, previous_state: Option<&ServiceState>) -> PathBuf {
        previous_state
            .map(|state| state.cwd.clone())
            .unwrap_or_else(|| self.cwd.clone())
    }

    fn daemon_args(&self, port: Option<u16>) -> Vec<String> {
        let mut args = vec![self.workflow_path.display().to_string()];
        if let Some(port) = port {
            args.push("--port".to_string());
            args.push(port.to_string());
        }
        args
    }

    fn display_command(&self, executable: &Path, port: Option<u16>) -> Vec<String> {
        let mut command = vec![executable.display().to_string()];
        command.extend(self.daemon_args(port));
        command
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceState {
    version: u32,
    workflow_path: PathBuf,
    cwd: PathBuf,
    pid: Option<u32>,
    status: StoredStatus,
    started_at_unix: Option<u64>,
    stopped_at_unix: Option<u64>,
    log_path: PathBuf,
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

pub(crate) async fn run(args: ServiceArgs) -> Result<(), Box<dyn Error>> {
    match args.command {
        ServiceCommand::Install(args) => {
            let target = ServiceTarget::load_for_dispatch(args.workflow, args.port)?;
            target.start("installed")?;
        }
        ServiceCommand::Uninstall(args) => {
            let target = ServiceTarget::load(args.workflow, None, false)?;
            target.uninstall()?;
        }
        ServiceCommand::Status(args) => {
            let target = ServiceTarget::load(args.workflow, None, false)?;
            target.print_status()?;
        }
        ServiceCommand::Logs(args) => {
            let target = ServiceTarget::load(args.workflow, None, false)?;
            target.print_logs(args.lines, args.follow)?;
        }
        ServiceCommand::Start(args) => {
            let target = ServiceTarget::load_for_dispatch(args.workflow, args.port)?;
            target.start("started")?;
        }
        ServiceCommand::Stop(args) => {
            let target = ServiceTarget::load(args.workflow, None, false)?;
            let _ = target.stop(false)?;
        }
        ServiceCommand::Restart(args) => {
            let target = ServiceTarget::load_for_dispatch(args.workflow, args.port)?;
            let _ = target.stop(false)?;
            target.start("restarted")?;
        }
    }
    Ok(())
}

fn resolve_workflow_path(workflow: Option<PathBuf>) -> io::Result<PathBuf> {
    let path = workflow.unwrap_or_else(|| PathBuf::from("WORKFLOW.md"));
    let absolute = if path.is_absolute() {
        path
    } else {
        env::current_dir()?.join(path)
    };
    match fs::canonicalize(&absolute) {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(absolute),
        Err(err) => Err(err),
    }
}

fn service_cwd_for_workflow(workflow_path: &Path, explicit_workflow: bool) -> io::Result<PathBuf> {
    if explicit_workflow {
        return Ok(workflow_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf());
    }
    env::current_dir()
}

fn load_dotenv_from_dir(dir: &Path) -> Result<(), Box<dyn Error>> {
    for ancestor in dir.ancestors() {
        let path = ancestor.join(".env");
        match dotenvy::from_path(&path) {
            Ok(_) => return Ok(()),
            Err(dotenvy::Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("failed to load {}: {err}", path.display()).into()),
        }
    }
    Ok(())
}

fn read_state(path: &Path) -> Result<Option<ServiceState>, Box<dyn Error>> {
    match fs::read_to_string(path) {
        Ok(body) => Ok(Some(serde_json::from_str(&body)?)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn write_state(path: &Path, state: &ServiceState) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(state)?))?;
    Ok(())
}

fn remove_state_file(path: &Path) -> Result<(), Box<dyn Error>> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn classify_state(state: &ServiceState) -> RuntimeStatus {
    match (state.status, state.pid) {
        (StoredStatus::Running, Some(pid)) if process_matches_state(pid, state) => {
            RuntimeStatus::Running
        }
        (StoredStatus::Running, Some(_)) => RuntimeStatus::Stale,
        _ => RuntimeStatus::Stopped,
    }
}

fn stale_service_group_cleanup_allowed(pid: u32) -> bool {
    !process_alive(pid)
}

fn service_dir_for_workflow(workflow_path: &Path) -> PathBuf {
    workflow_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".vik")
        .join("service")
}

fn service_name(workflow_path: &Path) -> String {
    let stem = workflow_path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(sanitize_name_part)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "workflow".to_string());
    format!(
        "{stem}-{:016x}",
        fnv1a64(workflow_path.to_string_lossy().as_bytes())
    )
}

fn sanitize_name_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn print_recent_lines(path: &Path, lines: usize) -> io::Result<()> {
    for line in recent_log_lines(path, lines)? {
        println!("{line}");
    }
    Ok(())
}

fn recent_log_lines(path: &Path, lines: usize) -> io::Result<Vec<String>> {
    if lines == 0 {
        return Ok(Vec::new());
    }

    let file = OpenOptions::new().read(true).open(path)?;
    let reader = BufReader::new(file);
    let mut recent = VecDeque::with_capacity(lines);
    for line in reader.lines() {
        if recent.len() == lines {
            recent.pop_front();
        }
        recent.push_back(line?);
    }
    Ok(recent.into_iter().collect())
}

fn follow_log(path: &Path) -> io::Result<()> {
    let mut offset = fs::metadata(path)?.len();
    loop {
        thread::sleep(Duration::from_secs(1));
        let mut file = OpenOptions::new().read(true).open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut chunk = String::new();
        file.read_to_string(&mut chunk)?;
        if !chunk.is_empty() {
            print!("{chunk}");
            io::stdout().flush()?;
            offset += chunk.len() as u64;
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(unix)]
fn detach_command(command: &mut Command) {
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
fn detach_command(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn process_group_alive(pid: u32) -> bool {
    let process_group = -(pid as libc::pid_t);
    let result = unsafe { libc::kill(process_group, 0) };
    if result == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn process_matches_state(pid: u32, state: &ServiceState) -> bool {
    if !process_alive(pid) {
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
    command_mentions_workflow(&command, &state.workflow_path)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

#[cfg(windows)]
fn process_matches_state(pid: u32, state: &ServiceState) -> bool {
    if !process_alive(pid) {
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
    command_mentions_workflow(
        String::from_utf8_lossy(&output.stdout).as_ref(),
        &state.workflow_path,
    )
}

fn command_mentions_workflow(command: &str, workflow_path: &Path) -> bool {
    command.contains(workflow_path.to_string_lossy().as_ref())
}

#[cfg(unix)]
fn terminate_pid(pid: u32) -> io::Result<()> {
    if !process_group_alive(pid) && !process_alive(pid) {
        return Ok(());
    }
    signal_service_process_group(pid, libc::SIGTERM)?;
    if wait_until_process_group_dead(pid, Duration::from_secs(5)) {
        return Ok(());
    }
    signal_service_process_group(pid, libc::SIGKILL)?;
    let _ = wait_until_process_group_dead(pid, Duration::from_secs(2));
    Ok(())
}

#[cfg(unix)]
fn terminate_stale_service_processes(pid: u32) -> io::Result<()> {
    if !process_group_alive(pid) {
        return Ok(());
    }
    signal_service_process_group(pid, libc::SIGTERM)?;
    if wait_until_process_group_dead(pid, Duration::from_secs(5)) {
        return Ok(());
    }
    signal_service_process_group(pid, libc::SIGKILL)?;
    let _ = wait_until_process_group_dead(pid, Duration::from_secs(2));
    Ok(())
}

#[cfg(unix)]
fn signal_service_process_group(pid: u32, signal: libc::c_int) -> io::Result<()> {
    // Services call setsid() on spawn, so the service pid is also the process group id.
    let process_group = -(pid as libc::pid_t);
    let result = unsafe { libc::kill(process_group, signal) };
    if result == 0 {
        return Ok(());
    }
    let group_err = io::Error::last_os_error();
    if group_err.raw_os_error() != Some(libc::ESRCH) || !process_alive(pid) {
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
fn terminate_pid(pid: u32) -> io::Result<()> {
    if !process_alive(pid) {
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
fn terminate_stale_service_processes(_pid: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn wait_until_process_group_dead(pid: u32, timeout: Duration) -> bool {
    let start = SystemTime::now();
    while process_group_alive(pid) {
        if start.elapsed().unwrap_or_default() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_is_ascii_and_stable() {
        let path = PathBuf::from("/tmp/Vik Workflow/WORKFLOW.md");

        let name = service_name(&path);

        assert_eq!(name, service_name(&path));
        assert!(name.starts_with("WORKFLOW-"));
        assert!(
            name.chars()
                .all(|ch| { ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' })
        );
    }

    #[test]
    fn daemon_args_include_workflow_and_optional_port() {
        let target = ServiceTarget {
            workflow_path: PathBuf::from("/tmp/vik/WORKFLOW.md"),
            service_dir: PathBuf::from("/tmp/vik/.vik/service"),
            state_path: PathBuf::from("/tmp/vik/.vik/service/state.json"),
            log_path: PathBuf::from("/tmp/vik/.vik/service/service.log"),
            cwd: PathBuf::from("/tmp/vik"),
            port: Some(3000),
        };

        assert_eq!(
            target.daemon_args(target.port),
            vec!["/tmp/vik/WORKFLOW.md", "--port", "3000"]
        );
    }

    #[test]
    fn effective_service_port_reuses_stored_port_without_override() {
        let state = service_state_with_port(Some(3000));
        let target = service_target_with_cwd(PathBuf::from("/tmp/vik"));

        assert_eq!(target.effective_port(Some(&state)), Some(3000));
    }

    #[test]
    fn effective_service_port_prefers_requested_override() {
        let state = service_state_with_port(Some(3000));
        let mut target = service_target_with_cwd(PathBuf::from("/tmp/vik"));
        target.port = Some(4000);

        assert_eq!(target.effective_port(Some(&state)), Some(4000));
    }

    #[test]
    fn effective_service_cwd_reuses_stored_cwd() {
        let target = service_target_with_cwd(PathBuf::from("/tmp/new-cwd"));
        let state = service_state_with_cwd(PathBuf::from("/tmp/installed-cwd"), None);

        assert_eq!(
            target.effective_cwd(Some(&state)),
            PathBuf::from("/tmp/installed-cwd")
        );
    }

    #[test]
    fn load_target_state_path_survives_workspace_root_change() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");
        write_workflow(&workflow_path, "work-a");
        let first = ServiceTarget::load(Some(workflow_path.clone()), None, false).unwrap();

        write_workflow(&workflow_path, "work-b");
        let second = ServiceTarget::load(Some(workflow_path.clone()), None, false).unwrap();

        assert_eq!(first.state_path, second.state_path);
        let expected_root = fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            first.service_dir,
            expected_root.join(".vik").join("service")
        );
    }

    #[test]
    fn management_target_does_not_parse_invalid_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");
        fs::write(&workflow_path, "---\ntracker: [\n---\nBody").unwrap();

        let target = ServiceTarget::load(Some(workflow_path.clone()), None, false).unwrap();

        assert_eq!(
            target.service_dir,
            fs::canonicalize(dir.path())
                .unwrap()
                .join(".vik")
                .join("service")
        );
    }

    #[test]
    fn management_target_survives_missing_workflow_file() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");

        let target = ServiceTarget::load(Some(workflow_path.clone()), None, false).unwrap();

        assert_eq!(target.workflow_path, workflow_path);
        assert_eq!(target.service_dir, dir.path().join(".vik").join("service"));
    }

    #[test]
    fn explicit_workflow_target_uses_workflow_dir_as_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");
        write_workflow(&workflow_path, "work");

        let target = ServiceTarget::load(Some(workflow_path), None, false).unwrap();

        assert_eq!(target.cwd, fs::canonicalize(dir.path()).unwrap());
    }

    #[test]
    fn command_match_requires_workflow_path() {
        let workflow_path = PathBuf::from("/tmp/vik/WORKFLOW.md");

        assert!(command_mentions_workflow(
            "vik /tmp/vik/WORKFLOW.md --port 3000",
            &workflow_path
        ));
        assert!(!command_mentions_workflow(
            "vik /tmp/other/WORKFLOW.md",
            &workflow_path
        ));
    }

    #[test]
    fn load_dotenv_from_dir_walks_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested").join("service");
        fs::create_dir_all(&nested).unwrap();
        let key = unique_env_key("SERVICE");
        fs::write(dir.path().join(".env"), format!("{key}=from_service_dir\n")).unwrap();

        load_dotenv_from_dir(&nested).unwrap();

        assert_eq!(env::var(key).unwrap(), "from_service_dir");
    }

    #[test]
    fn stopped_state_classifies_as_stopped_without_pid_probe() {
        let state = ServiceState {
            version: 1,
            workflow_path: PathBuf::from("/tmp/vik/WORKFLOW.md"),
            cwd: PathBuf::from("/tmp/vik"),
            pid: Some(999_999),
            status: StoredStatus::Stopped,
            started_at_unix: Some(1),
            stopped_at_unix: Some(2),
            log_path: PathBuf::from("/tmp/vik/service.log"),
            port: None,
            command: vec![],
        };

        assert_eq!(classify_state(&state), RuntimeStatus::Stopped);
    }

    #[test]
    fn stale_group_cleanup_skips_live_pid() {
        assert!(!stale_service_group_cleanup_allowed(std::process::id()));
    }

    fn service_state_with_port(port: Option<u16>) -> ServiceState {
        service_state_with_cwd(PathBuf::from("/tmp/vik"), port)
    }

    fn service_state_with_cwd(cwd: PathBuf, port: Option<u16>) -> ServiceState {
        ServiceState {
            version: 1,
            workflow_path: PathBuf::from("/tmp/vik/WORKFLOW.md"),
            cwd,
            pid: None,
            status: StoredStatus::Stopped,
            started_at_unix: Some(1),
            stopped_at_unix: Some(2),
            log_path: PathBuf::from("/tmp/vik/service.log"),
            port,
            command: vec![],
        }
    }

    fn service_target_with_cwd(cwd: PathBuf) -> ServiceTarget {
        ServiceTarget {
            workflow_path: PathBuf::from("/tmp/vik/WORKFLOW.md"),
            service_dir: PathBuf::from("/tmp/vik/.vik/service"),
            state_path: PathBuf::from("/tmp/vik/.vik/service/state.json"),
            log_path: PathBuf::from("/tmp/vik/.vik/service/service.log"),
            cwd,
            port: None,
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

    #[test]
    fn recent_log_lines_are_limited() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service.log");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();

        let lines = recent_log_lines(&path, 2).unwrap();

        assert_eq!(lines, ["two", "three"]);
    }

    #[test]
    fn recent_log_lines_supports_zero_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service.log");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();

        let lines = recent_log_lines(&path, 0).unwrap();

        assert!(lines.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn signal_service_process_group_stops_unix_descendants() {
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
        detach_command(&mut command);

        let mut child = command.spawn().unwrap();
        let service_pid = child.id();
        let child_pid = read_child_pid(&child_pid_path);

        signal_service_process_group(service_pid, libc::SIGTERM).unwrap();
        let _ = child.wait();

        assert!(wait_for_dead(child_pid, Duration::from_secs(2)));
    }

    #[cfg(unix)]
    #[test]
    fn terminate_stale_service_processes_stops_group_without_leader() {
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
        detach_command(&mut command);

        let mut child = command.spawn().unwrap();
        let service_pid = child.id();
        let child_pid = read_child_pid(&child_pid_path);
        let _ = child.wait();

        assert!(!process_alive(service_pid));
        assert!(process_alive(child_pid));

        terminate_stale_service_processes(service_pid).unwrap();

        assert!(wait_for_dead(child_pid, Duration::from_secs(2)));
    }

    #[cfg(unix)]
    #[test]
    fn terminate_pid_force_kills_unix_descendants_after_leader_exit() {
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
        detach_command(&mut command);

        let mut child = command.spawn().unwrap();
        let service_pid = child.id();
        let child_pid = read_child_pid(&child_pid_path);

        terminate_pid(service_pid).unwrap();
        let _ = child.wait();

        assert!(wait_for_dead(child_pid, Duration::from_secs(2)));
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
    fn wait_for_dead(pid: u32, timeout: Duration) -> bool {
        let start = SystemTime::now();
        while process_alive(pid) {
            if start.elapsed().unwrap_or_default() >= timeout {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
        true
    }
}
