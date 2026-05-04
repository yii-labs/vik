use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::env;
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use clap::{Args as ClapArgs, Subcommand};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
use tempfile::NamedTempFile;
use tokio::sync::{Mutex, mpsc};
use tokio::time;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use vik_agent::LocalAgentWorker;
use vik_core::{IssueDebugSnapshot, RuntimeSnapshot, TokenTotals};
use vik_http::{HttpState, serve};
use vik_orchestrator::Orchestrator;
use vik_tracker::{
    DEFAULT_LINEAR_ENDPOINT, LinearClient, LinearClientConfig, LinearIssueFilterConfig,
};
use vik_workflow::{WorkflowReloader, load_effective_workflow_with_env};

const REGISTRATION_ENV_KEYS: &[&str] = &[
    "CODEX_HOME",
    "GH_CONFIG_DIR",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GIT_SSH_COMMAND",
    "HOME",
    "LINEAR_API_KEY",
    "OPENAI_API_KEY",
    "PATH",
    "SSH_AUTH_SOCK",
    "USERPROFILE",
];

#[derive(Debug, ClapArgs)]
pub(crate) struct ServiceArgs {
    #[command(subcommand)]
    command: ServiceCommand,
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    #[command(hide = true)]
    Install(ServiceRunArgs),
    /// Remove service state and stop Vik if it is running.
    Uninstall,
    /// Print current service status.
    Status,
    /// Print recent service logs.
    Logs(LogsArgs),
    /// Start the Vik service in the background.
    Start(ServiceRunArgs),
    /// Stop a running Vik service.
    Stop,
    /// Stop then start the Vik service in the background.
    Restart(ServiceRunArgs),
}

#[derive(Debug, Clone, ClapArgs)]
struct ServiceRunArgs {
    /// Enable HTTP status server. Overrides server.port from WORKFLOW.md.
    #[arg(long)]
    port: Option<u16>,

    /// HTTP status server bind address. Defaults to 127.0.0.1.
    #[arg(long, alias = "host", value_name = "ADDR")]
    bind_address: Option<IpAddr>,
}

#[derive(Debug, Clone, ClapArgs)]
struct LogsArgs {
    /// Number of recent lines to print.
    #[arg(long, default_value_t = 100)]
    lines: usize,

    /// Continue printing appended log output.
    #[arg(long)]
    follow: bool,
}

#[derive(Debug, Clone, ClapArgs)]
pub(crate) struct DaemonArgs {
    #[arg(long, hide = true)]
    service_dir: Option<PathBuf>,

    /// Register a workflow before the daemon starts.
    #[arg(short = 'w', long = "workflow", value_name = "WORKFLOW")]
    pub(crate) workflows: Vec<PathBuf>,

    /// Enable HTTP status server. Overrides server.port from WORKFLOW.md.
    #[arg(long)]
    pub(crate) port: Option<u16>,

    /// HTTP status server bind address. Defaults to 127.0.0.1.
    #[arg(long, alias = "host", value_name = "ADDR")]
    pub(crate) bind_address: Option<IpAddr>,
}

#[derive(Debug, Clone)]
struct ServicePaths {
    service_dir: PathBuf,
    state_path: PathBuf,
    log_path: PathBuf,
    registry_path: PathBuf,
}

impl ServicePaths {
    fn load(service_dir: Option<PathBuf>) -> io::Result<Self> {
        let service_dir = resolve_service_dir(service_dir)?;
        Ok(Self::from_dir(service_dir))
    }

    fn from_dir(service_dir: PathBuf) -> Self {
        Self {
            state_path: service_dir.join("service.json"),
            log_path: service_dir.join("service.log"),
            registry_path: service_dir.join("workflows.json"),
            service_dir,
        }
    }
}

#[derive(Debug)]
struct ServiceTarget {
    paths: ServicePaths,
    port: Option<u16>,
    bind_address: Option<IpAddr>,
    legacy_workdir: PathBuf,
}

impl ServiceTarget {
    fn load(port: Option<u16>, bind_address: Option<IpAddr>) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            paths: ServicePaths::load(None)?,
            port,
            bind_address,
            legacy_workdir: env::current_dir()?,
        })
    }

    fn start(&self, verb: &str) -> Result<(), Box<dyn Error>> {
        let previous_state = read_state(&self.paths.state_path)?;
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
        let bind_address = self.effective_bind_address(previous_state.as_ref());
        let cwd = previous_state
            .as_ref()
            .map(|state| state.cwd.clone())
            .unwrap_or(env::current_dir()?);
        fs::create_dir_all(&self.paths.service_dir)?;
        let executable = env::current_exe()?;
        let mut command = Command::new(&executable);
        for arg in self.daemon_args(port, bind_address) {
            command.arg(arg);
        }
        command.current_dir(&cwd);
        detach_command(&mut command);

        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.paths.log_path)?;
        let err_log = log.try_clone()?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err_log));

        let child = command.spawn()?;
        let pid = child.id();
        let state = ServiceState {
            version: 2,
            service_dir: self.paths.service_dir.clone(),
            registry_path: self.paths.registry_path.clone(),
            cwd,
            pid: Some(pid),
            status: StoredStatus::Running,
            started_at_unix: Some(now_unix()),
            stopped_at_unix: None,
            log_path: self.paths.log_path.clone(),
            port,
            bind_address: bind_address.map(|addr| addr.to_string()),
            command: self.display_command(&executable, port, bind_address),
        };
        write_state(&self.paths.state_path, &state)?;
        println!(
            "service {verb}: pid={} state={} log={}",
            pid,
            self.paths.state_path.display(),
            self.paths.log_path.display()
        );
        Ok(())
    }

    fn stop(&self, remove_state: bool) -> Result<bool, Box<dyn Error>> {
        let Some(mut state) = read_state(&self.paths.state_path)? else {
            if stop_legacy_services_in_workdir(&self.legacy_workdir, remove_state)? > 0 {
                return Ok(true);
            }
            println!("service not installed: {}", self.paths.state_path.display());
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
                println!(
                    "service already stopped: {}",
                    self.paths.state_path.display()
                );
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
            remove_state_file(&self.paths.state_path)?;
        } else {
            write_state(&self.paths.state_path, &state)?;
        }
        Ok(true)
    }

    fn uninstall(&self) -> Result<(), Box<dyn Error>> {
        let _ = self.stop(true)?;
        let _ = stop_legacy_services_in_workdir(&self.legacy_workdir, true)?;
        remove_state_file(&self.paths.registry_path)?;
        println!("service uninstalled: {}", self.paths.state_path.display());
        Ok(())
    }

    fn print_status(&self) -> Result<(), Box<dyn Error>> {
        let registry = read_registry(&self.paths.registry_path)?;
        let Some(state) = read_state(&self.paths.state_path)? else {
            if print_legacy_status_in_workdir(&self.legacy_workdir)? {
                print_registry_status(&registry);
                return Ok(());
            }
            println!("not installed: {}", self.paths.state_path.display());
            print_registry_status(&registry);
            return Ok(());
        };

        match classify_state(&state) {
            RuntimeStatus::Running => println!(
                "running: pid={} workflows={} log={}",
                state.pid.unwrap_or_default(),
                registry.workflows.len(),
                state.log_path.display()
            ),
            RuntimeStatus::Stopped => println!(
                "stopped: workflows={} log={}",
                registry.workflows.len(),
                state.log_path.display()
            ),
            RuntimeStatus::Stale => println!(
                "stale: pid={} workflows={} log={}",
                state.pid.unwrap_or_default(),
                registry.workflows.len(),
                state.log_path.display()
            ),
        }
        print_registry_status(&registry);
        Ok(())
    }

    fn print_logs(&self, lines: usize, follow: bool) -> Result<(), Box<dyn Error>> {
        let path = read_state(&self.paths.state_path)?
            .map(|state| state.log_path)
            .unwrap_or_else(|| self.paths.log_path.clone());
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

    fn effective_bind_address(&self, previous_state: Option<&ServiceState>) -> Option<IpAddr> {
        self.bind_address.or_else(|| {
            previous_state
                .and_then(|state| state.bind_address.as_ref())
                .and_then(|addr| addr.parse().ok())
        })
    }

    fn daemon_args(&self, port: Option<u16>, bind_address: Option<IpAddr>) -> Vec<String> {
        let mut args = vec![
            "daemon".to_string(),
            "--service-dir".to_string(),
            self.paths.service_dir.display().to_string(),
        ];
        if let Some(port) = port {
            args.push("--port".to_string());
            args.push(port.to_string());
        }
        if let Some(bind_address) = bind_address {
            args.push("--bind-address".to_string());
            args.push(bind_address.to_string());
        }
        args
    }

    fn display_command(
        &self,
        executable: &Path,
        port: Option<u16>,
        bind_address: Option<IpAddr>,
    ) -> Vec<String> {
        let mut command = vec![executable.display().to_string()];
        command.extend(self.daemon_args(port, bind_address));
        command
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceState {
    version: u32,
    service_dir: PathBuf,
    registry_path: PathBuf,
    cwd: PathBuf,
    pid: Option<u32>,
    status: StoredStatus,
    started_at_unix: Option<u64>,
    stopped_at_unix: Option<u64>,
    log_path: PathBuf,
    port: Option<u16>,
    bind_address: Option<String>,
    command: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyServiceState {
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

#[derive(Debug)]
struct LegacyServiceEntry {
    path: PathBuf,
    state: LegacyServiceState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RegisteredWorkflow {
    workflow_path: PathBuf,
    cwd: PathBuf,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    registration_env: HashMap<String, String>,
    registered_at_unix: u64,
}

impl RegisteredWorkflow {
    fn runtime_equivalent(&self, other: &Self) -> bool {
        self.workflow_path == other.workflow_path
            && self.cwd == other.cwd
            && self.registration_env == other.registration_env
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WorkflowRegistry {
    version: u32,
    workflows: Vec<RegisteredWorkflow>,
}

impl Default for WorkflowRegistry {
    fn default() -> Self {
        Self {
            version: 1,
            workflows: Vec::new(),
        }
    }
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
            register_current_workflow_if_present()?;
            let target = ServiceTarget::load(args.port, args.bind_address)?;
            target.start("installed")?;
        }
        ServiceCommand::Uninstall => {
            let target = ServiceTarget::load(None, None)?;
            target.uninstall()?;
        }
        ServiceCommand::Status => {
            let target = ServiceTarget::load(None, None)?;
            target.print_status()?;
        }
        ServiceCommand::Logs(args) => {
            let target = ServiceTarget::load(None, None)?;
            target.print_logs(args.lines, args.follow)?;
        }
        ServiceCommand::Start(args) => {
            register_current_workflow_if_present()?;
            let target = ServiceTarget::load(args.port, args.bind_address)?;
            target.start("started")?;
        }
        ServiceCommand::Stop => {
            let target = ServiceTarget::load(None, None)?;
            let _ = target.stop(false)?;
        }
        ServiceCommand::Restart(args) => {
            register_current_workflow_if_present()?;
            let target = ServiceTarget::load(args.port, args.bind_address)?;
            let _ = target.stop(false)?;
            target.start("restarted")?;
        }
    }
    Ok(())
}

pub(crate) fn register_workflow_and_start_service(
    workflow: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let target = ServiceTarget::load(None, None)?;
    register_workflow_in_registry(&target.paths, workflow)?;
    target.start("started")?;
    Ok(())
}

pub(crate) async fn run_daemon(args: DaemonArgs) -> Result<(), Box<dyn Error>> {
    let paths = ServicePaths::load(args.service_dir)?;
    fs::create_dir_all(&paths.service_dir)?;
    for workflow in args.workflows {
        register_workflow_in_registry(&paths, Some(workflow))?;
    }

    let log_dir = daemon_log_dir(&paths);
    let _log_guard = init_logging(&log_dir)?;
    tracing::info!(
        service_dir=%paths.service_dir.display(),
        log_dir=%log_dir.display(),
        "service_daemon outcome=started"
    );

    run_workflow_center(paths, args.port, args.bind_address).await
}

fn register_current_workflow_if_present() -> Result<(), Box<dyn Error>> {
    let workflow_path = env::current_dir()?.join("WORKFLOW.md");
    if workflow_path.is_file() {
        let paths = ServicePaths::load(None)?;
        register_workflow_in_registry(&paths, Some(workflow_path))?;
    }
    Ok(())
}

fn register_workflow_in_registry(
    paths: &ServicePaths,
    workflow: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    register_workflow_in_registry_with_env(paths, workflow, env::vars().collect())
}

fn register_workflow_in_registry_with_env(
    paths: &ServicePaths,
    workflow: Option<PathBuf>,
    process_env: HashMap<String, String>,
) -> Result<(), Box<dyn Error>> {
    let explicit_workflow = workflow.is_some();
    let management_path = resolve_workflow_path(workflow)?;
    let cwd = service_cwd_for_workflow(&management_path, explicit_workflow)?;
    let runtime_env = workflow_env_from_dir_with_process_env(&cwd, process_env.clone())?;
    let loaded = load_effective_workflow_with_env(Some(management_path), &runtime_env)?;
    loaded.config.validate_for_dispatch()?;
    let registration_env = capture_registration_env(&loaded.definition.config, &runtime_env);
    let workflow_path = fs::canonicalize(&loaded.definition.path)?;
    let cwd = if explicit_workflow {
        workflow_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    } else {
        env::current_dir()?
    };

    let _registry_lock = lock_registry(&paths.registry_path)?;
    let mut registry = read_registry(&paths.registry_path)?;
    let workflow_record = RegisteredWorkflow {
        workflow_path: workflow_path.clone(),
        cwd,
        registration_env,
        registered_at_unix: now_unix(),
    };
    let already_registered = if let Some(existing) = registry
        .workflows
        .iter_mut()
        .find(|entry| entry.workflow_path == workflow_path)
    {
        *existing = workflow_record;
        true
    } else {
        registry.workflows.push(workflow_record);
        false
    };
    registry
        .workflows
        .sort_by(|left, right| left.workflow_path.cmp(&right.workflow_path));
    write_registry(&paths.registry_path, &registry)?;

    if already_registered {
        println!("workflow already registered: {}", workflow_path.display());
    } else {
        println!("workflow registered: {}", workflow_path.display());
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

#[cfg(test)]
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

#[cfg(test)]
fn workflow_env_from_dir(dir: &Path) -> Result<HashMap<String, String>, Box<dyn Error>> {
    workflow_env_from_dir_with_process_env(dir, env::vars().collect())
}

fn workflow_env_from_dir_with_process_env(
    dir: &Path,
    mut env_map: HashMap<String, String>,
) -> Result<HashMap<String, String>, Box<dyn Error>> {
    for ancestor in dir.ancestors() {
        let path = ancestor.join(".env");
        match dotenvy::from_path_iter(&path) {
            Ok(iter) => {
                for item in iter {
                    let (key, value) =
                        item.map_err(|err| format!("failed to load {}: {err}", path.display()))?;
                    env_map.entry(key).or_insert(value);
                }
                return Ok(env_map);
            }
            Err(dotenvy::Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("failed to load {}: {err}", path.display()).into()),
        }
    }
    Ok(env_map)
}

type WorkflowOrchestrator = Orchestrator<LinearClient, LocalAgentWorker<LinearClient>>;

struct WorkflowRuntime {
    registration: RegisteredWorkflow,
    orchestrator: Arc<WorkflowOrchestrator>,
    task: tokio::task::JoinHandle<()>,
    server_port: Option<u16>,
}

async fn run_workflow_center(
    paths: ServicePaths,
    explicit_port: Option<u16>,
    bind_address: Option<IpAddr>,
) -> Result<(), Box<dyn Error>> {
    let runtimes: Arc<Mutex<BTreeMap<PathBuf, WorkflowRuntime>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let (refresh_tx, mut refresh_rx) = mpsc::unbounded_channel();
    let mut http_started = false;
    if let Some(port) = explicit_port {
        start_http_server(
            Arc::clone(&runtimes),
            refresh_tx.clone(),
            bind_address,
            port,
        )
        .await?;
        http_started = true;
    }

    let mut interval = time::interval(Duration::from_secs(2));
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                reconcile_registered_workflows(&paths.registry_path, Arc::clone(&runtimes)).await?;
                reap_finished_workflows(Arc::clone(&runtimes)).await;
                if !http_started
                    && let Some(port) = preferred_runtime_port(Arc::clone(&runtimes)).await
                {
                    start_http_server(Arc::clone(&runtimes), refresh_tx.clone(), bind_address, port).await?;
                    http_started = true;
                }
            }
            Some(()) = refresh_rx.recv() => {
                broadcast_refresh(Arc::clone(&runtimes)).await;
            }
            result = tokio::signal::ctrl_c() => {
                result?;
                tracing::info!("service_daemon outcome=stopping signal=ctrl_c");
                abort_workflows(Arc::clone(&runtimes)).await;
                return Ok(());
            }
        }
    }
}

async fn reconcile_registered_workflows(
    registry_path: &Path,
    runtimes: Arc<Mutex<BTreeMap<PathBuf, WorkflowRuntime>>>,
) -> Result<(), Box<dyn Error>> {
    let registry = match read_registry(registry_path) {
        Ok(registry) => registry,
        Err(err) => {
            tracing::warn!(
                registry=%registry_path.display(),
                error=%err,
                "workflow_registry outcome=read_failed"
            );
            return Ok(());
        }
    };
    for workflow in registry.workflows {
        let runtime_is_current = runtimes
            .lock()
            .await
            .get(&workflow.workflow_path)
            .is_some_and(|runtime| runtime.registration.runtime_equivalent(&workflow));
        if runtime_is_current {
            continue;
        }
        let runtime = match start_workflow_runtime(&workflow) {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::error!(
                    workflow=%workflow.workflow_path.display(),
                    error=%err,
                    "workflow_runtime outcome=start_failed"
                );
                continue;
            }
        };
        let previous = runtimes
            .lock()
            .await
            .insert(workflow.workflow_path.clone(), runtime);
        if let Some(previous) = previous {
            previous.task.abort();
            let aborted_workers = previous.orchestrator.abort_running_workers().await;
            tracing::info!(
                workflow=%workflow.workflow_path.display(),
                aborted_workers,
                "workflow_runtime outcome=restarted reason=registration_changed"
            );
        } else {
            tracing::info!(
                workflow=%workflow.workflow_path.display(),
                "workflow_runtime outcome=started"
            );
        }
    }
    Ok(())
}

fn start_workflow_runtime(
    workflow: &RegisteredWorkflow,
) -> Result<WorkflowRuntime, Box<dyn Error>> {
    let runtime_env = workflow_runtime_env(workflow)?;
    let reloader =
        WorkflowReloader::start_with_env(Some(workflow.workflow_path.clone()), runtime_env)?;
    let loaded = reloader.current().clone();
    loaded.config.validate_for_dispatch()?;
    let server_port = loaded.config.server.as_ref().map(|server| server.port);

    let tracker_config = LinearClientConfig::new(
        if loaded.config.tracker.endpoint.is_empty() {
            DEFAULT_LINEAR_ENDPOINT
        } else {
            &loaded.config.tracker.endpoint
        },
        &loaded.config.tracker.api_key,
        &loaded.config.tracker.project_slug,
        loaded.config.tracker.active_states.clone(),
    )
    .with_filter(LinearIssueFilterConfig::new(
        loaded.config.tracker.filter.assignees.clone(),
        loaded.config.tracker.filter.tags.clone(),
    ));
    let tracker = Arc::new(LinearClient::new(tracker_config)?);
    let worker = Arc::new(LocalAgentWorker::new(Arc::clone(&tracker)));
    let orchestrator = Arc::new(Orchestrator::new(Arc::clone(&tracker), worker, reloader));
    let run_orchestrator = Arc::clone(&orchestrator);
    let workflow_path = workflow.workflow_path.clone();
    let task = tokio::spawn(async move {
        if let Err(err) = run_orchestrator.run_forever().await {
            tracing::error!(
                workflow=%workflow_path.display(),
                error=%err,
                "workflow_runtime outcome=failed"
            );
        }
    });

    Ok(WorkflowRuntime {
        registration: workflow.clone(),
        orchestrator,
        task,
        server_port,
    })
}

fn daemon_log_dir(paths: &ServicePaths) -> PathBuf {
    let Ok(registry) = read_registry(&paths.registry_path) else {
        return paths.service_dir.join("logs");
    };
    for workflow in registry.workflows {
        let Ok(runtime_env) = workflow_runtime_env(&workflow) else {
            continue;
        };
        let Ok(loaded) =
            load_effective_workflow_with_env(Some(workflow.workflow_path.clone()), &runtime_env)
        else {
            continue;
        };
        return loaded.config.logging.dir;
    }
    paths.service_dir.join("logs")
}

fn workflow_runtime_env(
    workflow: &RegisteredWorkflow,
) -> Result<HashMap<String, String>, Box<dyn Error>> {
    workflow_runtime_env_with_process_env(workflow, env::vars().collect())
}

fn workflow_runtime_env_with_process_env(
    workflow: &RegisteredWorkflow,
    process_env: HashMap<String, String>,
) -> Result<HashMap<String, String>, Box<dyn Error>> {
    let mut runtime_env = workflow_env_from_dir_with_process_env(&workflow.cwd, process_env)?;
    for (key, value) in &workflow.registration_env {
        runtime_env.insert(key.clone(), value.clone());
    }
    Ok(runtime_env)
}

fn capture_registration_env(
    config: &serde_yaml::Mapping,
    process_env: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut keys: BTreeSet<String> = REGISTRATION_ENV_KEYS
        .iter()
        .map(|key| (*key).to_string())
        .collect();
    collect_config_env_refs_from_mapping(config, &mut keys);
    keys.into_iter()
        .filter_map(|key| process_env.get(&key).map(|value| (key, value.clone())))
        .collect()
}

fn collect_config_env_refs_from_mapping(
    mapping: &serde_yaml::Mapping,
    keys: &mut BTreeSet<String>,
) {
    for value in mapping.values() {
        collect_config_env_refs_from_value(value, keys);
    }
}

fn collect_config_env_refs_from_value(value: &YamlValue, keys: &mut BTreeSet<String>) {
    match value {
        YamlValue::String(raw) => {
            collect_env_refs_from_string(raw, keys);
            if raw.starts_with("~/") {
                keys.insert("HOME".to_string());
            }
        }
        YamlValue::Sequence(values) => {
            for value in values {
                collect_config_env_refs_from_value(value, keys);
            }
        }
        YamlValue::Mapping(mapping) => collect_config_env_refs_from_mapping(mapping, keys),
        _ => {}
    }
}

fn collect_env_refs_from_string(raw: &str, keys: &mut BTreeSet<String>) {
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            continue;
        }

        if chars.next_if_eq(&'{').is_some() {
            let mut var = String::new();
            let mut closed = false;
            for ch in chars.by_ref() {
                if ch == '}' {
                    closed = true;
                    break;
                }
                var.push(ch);
            }
            if closed && valid_env_ref(&var) {
                keys.insert(var);
            }
            continue;
        }

        let Some(&first) = chars.peek() else {
            continue;
        };
        if !env_ref_start(first) {
            continue;
        }
        let mut var = String::new();
        while let Some(&ch) = chars.peek() {
            if !env_ref_continue(ch) {
                break;
            }
            var.push(ch);
            chars.next();
        }
        keys.insert(var);
    }
}

fn valid_env_ref(raw: &str) -> bool {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    env_ref_start(first) && chars.all(env_ref_continue)
}

fn env_ref_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn env_ref_continue(ch: char) -> bool {
    env_ref_start(ch) || ch.is_ascii_digit()
}

async fn reap_finished_workflows(runtimes: Arc<Mutex<BTreeMap<PathBuf, WorkflowRuntime>>>) {
    let finished = {
        let mut runtimes = runtimes.lock().await;
        let finished_paths: Vec<_> = runtimes
            .iter()
            .filter_map(|(workflow_path, runtime)| {
                runtime.task.is_finished().then_some(workflow_path.clone())
            })
            .collect();
        finished_paths
            .into_iter()
            .filter_map(|workflow_path| {
                runtimes
                    .remove(&workflow_path)
                    .map(|runtime| (workflow_path, runtime))
            })
            .collect::<Vec<_>>()
    };
    for (workflow_path, runtime) in finished {
        let aborted_workers = runtime.orchestrator.abort_running_workers().await;
        tracing::warn!(
            workflow=%workflow_path.display(),
            aborted_workers,
            "workflow_runtime outcome=exited"
        );
    }
}

async fn preferred_runtime_port(
    runtimes: Arc<Mutex<BTreeMap<PathBuf, WorkflowRuntime>>>,
) -> Option<u16> {
    let runtimes = runtimes.lock().await;
    runtimes.values().find_map(|runtime| runtime.server_port)
}

async fn start_http_server(
    runtimes: Arc<Mutex<BTreeMap<PathBuf, WorkflowRuntime>>>,
    refresh_tx: mpsc::UnboundedSender<()>,
    bind_address: Option<IpAddr>,
    port: u16,
) -> Result<(), Box<dyn Error>> {
    let snapshot_runtimes = Arc::clone(&runtimes);
    let issue_runtimes = Arc::clone(&runtimes);
    let addr = SocketAddr::new(
        bind_address.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        port,
    );
    let bound = serve(
        addr,
        HttpState {
            snapshot: Arc::new(move || {
                let runtimes = Arc::clone(&snapshot_runtimes);
                Box::pin(async move { aggregate_snapshot(runtimes).await })
            }),
            issue: Arc::new(move |identifier| {
                let runtimes = Arc::clone(&issue_runtimes);
                Box::pin(async move { aggregate_issue_debug(runtimes, identifier).await })
            }),
            refresh_tx,
        },
    )
    .await?;
    tracing::info!(addr=%bound, "http_server outcome=started");
    Ok(())
}

async fn aggregate_snapshot(
    runtimes: Arc<Mutex<BTreeMap<PathBuf, WorkflowRuntime>>>,
) -> RuntimeSnapshot {
    let orchestrators: Vec<_> = runtimes
        .lock()
        .await
        .values()
        .map(|runtime| Arc::clone(&runtime.orchestrator))
        .collect();
    let mut aggregate = RuntimeSnapshot {
        generated_at: Utc::now(),
        counts: BTreeMap::from([("running".to_string(), 0), ("retrying".to_string(), 0)]),
        running: Vec::new(),
        retrying: Vec::new(),
        codex_totals: TokenTotals::default(),
        rate_limits: None,
    };
    for orchestrator in orchestrators {
        let snapshot = orchestrator.snapshot().await;
        for (key, count) in snapshot.counts {
            *aggregate.counts.entry(key).or_insert(0) += count;
        }
        aggregate.running.extend(snapshot.running);
        aggregate.retrying.extend(snapshot.retrying);
        aggregate.codex_totals.input_tokens += snapshot.codex_totals.input_tokens;
        aggregate.codex_totals.output_tokens += snapshot.codex_totals.output_tokens;
        aggregate.codex_totals.total_tokens += snapshot.codex_totals.total_tokens;
        aggregate.codex_totals.seconds_running += snapshot.codex_totals.seconds_running;
        if aggregate.rate_limits.is_none() {
            aggregate.rate_limits = snapshot.rate_limits;
        }
    }
    aggregate
}

async fn aggregate_issue_debug(
    runtimes: Arc<Mutex<BTreeMap<PathBuf, WorkflowRuntime>>>,
    issue_identifier: String,
) -> Option<IssueDebugSnapshot> {
    let orchestrators: Vec<_> = runtimes
        .lock()
        .await
        .values()
        .map(|runtime| Arc::clone(&runtime.orchestrator))
        .collect();
    for orchestrator in orchestrators {
        if let Some(snapshot) = orchestrator.issue_debug(&issue_identifier).await {
            return Some(snapshot);
        }
    }
    None
}

async fn broadcast_refresh(runtimes: Arc<Mutex<BTreeMap<PathBuf, WorkflowRuntime>>>) {
    let senders: Vec<_> = runtimes
        .lock()
        .await
        .values()
        .map(|runtime| runtime.orchestrator.refresh_sender())
        .collect();
    for sender in senders {
        let _ = sender.send(());
    }
}

async fn abort_workflows(runtimes: Arc<Mutex<BTreeMap<PathBuf, WorkflowRuntime>>>) {
    let orchestrators = {
        let runtimes = runtimes.lock().await;
        let mut orchestrators = Vec::with_capacity(runtimes.len());
        for runtime in runtimes.values() {
            runtime.task.abort();
            orchestrators.push(Arc::clone(&runtime.orchestrator));
        }
        orchestrators
    };
    for orchestrator in orchestrators {
        let _ = orchestrator.abort_running_workers().await;
    }
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

fn read_registry(path: &Path) -> Result<WorkflowRegistry, Box<dyn Error>> {
    match fs::read_to_string(path) {
        Ok(body) => Ok(serde_json::from_str(&body)?),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(WorkflowRegistry::default()),
        Err(err) => Err(err.into()),
    }
}

fn write_registry(path: &Path, registry: &WorkflowRegistry) -> Result<(), Box<dyn Error>> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let body = format!("{}\n", serde_json::to_string_pretty(registry)?);
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(body.as_bytes())?;
    temp.as_file_mut().sync_all()?;
    temp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

fn lock_registry(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_path = registry_lock_path(path);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock_exclusive()?;
    Ok(file)
}

fn registry_lock_path(path: &Path) -> PathBuf {
    let lock_file_name = path
        .file_name()
        .map(|name| format!("{}.lock", name.to_string_lossy()))
        .unwrap_or_else(|| "workflows.json.lock".to_string());
    path.with_file_name(lock_file_name)
}

fn print_registry_status(registry: &WorkflowRegistry) {
    for workflow in &registry.workflows {
        println!("workflow: {}", workflow.workflow_path.display());
    }
}

fn print_legacy_status_in_workdir(workdir: &Path) -> Result<bool, Box<dyn Error>> {
    let entries = legacy_service_entries_in_workdir(workdir)?;
    if entries.is_empty() {
        return Ok(false);
    }

    for entry in entries {
        match classify_legacy_state(&entry.state) {
            RuntimeStatus::Running => println!(
                "legacy running: pid={} workflow={} log={}",
                entry.state.pid.unwrap_or_default(),
                entry.state.workflow_path.display(),
                entry.state.log_path.display()
            ),
            RuntimeStatus::Stopped => println!(
                "legacy stopped: workflow={} log={}",
                entry.state.workflow_path.display(),
                entry.state.log_path.display()
            ),
            RuntimeStatus::Stale => println!(
                "legacy stale: pid={} workflow={} log={}",
                entry.state.pid.unwrap_or_default(),
                entry.state.workflow_path.display(),
                entry.state.log_path.display()
            ),
        }
    }
    Ok(true)
}

fn stop_legacy_services_in_workdir(
    workdir: &Path,
    remove_state: bool,
) -> Result<usize, Box<dyn Error>> {
    let entries = legacy_service_entries_in_workdir(workdir)?;
    let count = entries.len();
    for mut entry in entries {
        match classify_legacy_state(&entry.state) {
            RuntimeStatus::Running => {
                let pid = entry.state.pid.unwrap_or_default();
                terminate_pid(pid)?;
                entry.state.status = StoredStatus::Stopped;
                entry.state.stopped_at_unix = Some(now_unix());
                println!("legacy service stopped: pid={pid}");
            }
            RuntimeStatus::Stopped => {
                println!("legacy service already stopped: {}", entry.path.display());
            }
            RuntimeStatus::Stale => {
                let pid = entry.state.pid.unwrap_or_default();
                if pid != 0 && stale_service_group_cleanup_allowed(pid) {
                    terminate_stale_service_processes(pid)?;
                }
                entry.state.status = StoredStatus::Stopped;
                entry.state.stopped_at_unix = Some(now_unix());
                println!("legacy service stale: pid={pid} is not running");
            }
        }

        if remove_state {
            remove_state_file(&entry.path)?;
        } else {
            write_legacy_state(&entry.path, &entry.state)?;
        }
    }
    Ok(count)
}

fn legacy_service_entries_in_workdir(
    workdir: &Path,
) -> Result<Vec<LegacyServiceEntry>, Box<dyn Error>> {
    let mut entries = Vec::new();
    for path in legacy_service_state_paths_in_workdir(workdir)? {
        if let Some(state) = read_legacy_state(&path)? {
            entries.push(LegacyServiceEntry { path, state });
        }
    }
    Ok(entries)
}

fn legacy_service_state_paths_in_workdir(workdir: &Path) -> io::Result<Vec<PathBuf>> {
    let service_dir = workdir.join(".vik").join("service");
    let mut paths = Vec::new();
    let entries = match fs::read_dir(&service_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(paths),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let path = entry?.path();
        if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        if matches!(
            path.file_name().and_then(|value| value.to_str()),
            Some("service.json" | "workflows.json")
        ) {
            continue;
        }
        paths.push(path);
    }
    paths.sort();
    Ok(paths)
}

fn read_legacy_state(path: &Path) -> Result<Option<LegacyServiceState>, Box<dyn Error>> {
    match fs::read_to_string(path) {
        Ok(body) => match serde_json::from_str(&body) {
            Ok(state) => Ok(Some(state)),
            Err(_) => Ok(None),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn write_legacy_state(path: &Path, state: &LegacyServiceState) -> Result<(), Box<dyn Error>> {
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

fn classify_legacy_state(state: &LegacyServiceState) -> RuntimeStatus {
    match (state.status, state.pid) {
        (StoredStatus::Running, Some(pid)) if process_matches_legacy_state(pid, state) => {
            RuntimeStatus::Running
        }
        (StoredStatus::Running, Some(_)) => RuntimeStatus::Stale,
        _ => RuntimeStatus::Stopped,
    }
}

fn stale_service_group_cleanup_allowed(pid: u32) -> bool {
    !process_alive(pid)
}

fn resolve_service_dir(service_dir: Option<PathBuf>) -> io::Result<PathBuf> {
    let path = service_dir
        .or_else(|| env::var_os("VIK_SERVICE_DIR").map(PathBuf::from))
        .unwrap_or_else(|| home_dir().join(".vik").join("service"));
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
}

fn init_logging(log_dir: &Path) -> Result<WorkerGuard, Box<dyn Error>> {
    fs::create_dir_all(log_dir)?;
    let file_appender = tracing_appender::rolling::daily(log_dir, "vik.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
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

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();
    Ok(guard)
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
    command_mentions_service_dir(&command, &state.service_dir)
}

#[cfg(unix)]
fn process_matches_legacy_state(pid: u32, state: &LegacyServiceState) -> bool {
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
    command_mentions_service_dir(
        String::from_utf8_lossy(&output.stdout).as_ref(),
        &state.service_dir,
    )
}

#[cfg(windows)]
fn process_matches_legacy_state(pid: u32, state: &LegacyServiceState) -> bool {
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

fn command_mentions_service_dir(command: &str, service_dir: &Path) -> bool {
    command.contains(service_dir.to_string_lossy().as_ref())
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
    fn service_paths_use_central_files() {
        let paths = ServicePaths::from_dir(PathBuf::from("/tmp/vik-service"));

        assert_eq!(
            paths.state_path,
            PathBuf::from("/tmp/vik-service/service.json")
        );
        assert_eq!(
            paths.log_path,
            PathBuf::from("/tmp/vik-service/service.log")
        );
        assert_eq!(
            paths.registry_path,
            PathBuf::from("/tmp/vik-service/workflows.json")
        );
    }

    #[test]
    fn daemon_args_include_service_dir_and_optional_status_flags() {
        let mut target = service_target();
        target.port = Some(3000);
        target.bind_address = Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

        assert_eq!(
            target.daemon_args(target.port, target.bind_address),
            vec![
                "daemon",
                "--service-dir",
                "/tmp/vik/.vik/service",
                "--port",
                "3000",
                "--bind-address",
                "0.0.0.0"
            ]
        );
    }

    #[test]
    fn effective_service_port_reuses_stored_port_without_override() {
        let state = service_state_with_port(Some(3000));
        let target = service_target();

        assert_eq!(target.effective_port(Some(&state)), Some(3000));
    }

    #[test]
    fn effective_service_port_prefers_requested_override() {
        let state = service_state_with_port(Some(3000));
        let mut target = service_target();
        target.port = Some(4000);

        assert_eq!(target.effective_port(Some(&state)), Some(4000));
    }

    #[test]
    fn effective_bind_address_reuses_stored_address_without_override() {
        let mut state = service_state_with_port(None);
        state.bind_address = Some("0.0.0.0".to_string());
        let target = service_target();

        assert_eq!(
            target.effective_bind_address(Some(&state)),
            Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
        );
    }

    #[test]
    fn register_workflow_writes_central_registry() {
        let dir = tempfile::tempdir().unwrap();
        let service_dir = dir.path().join("service");
        let workflow_path = dir.path().join("WORKFLOW.md");
        write_workflow(&workflow_path, "work");
        let paths = ServicePaths::from_dir(service_dir);

        register_workflow_in_registry(&paths, Some(workflow_path.clone())).unwrap();

        let registry = read_registry(&paths.registry_path).unwrap();
        assert_eq!(registry.workflows.len(), 1);
        assert_eq!(
            registry.workflows[0].workflow_path,
            fs::canonicalize(workflow_path).unwrap()
        );
        assert_eq!(
            registry.workflows[0].cwd,
            fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[test]
    fn daemon_log_dir_uses_registered_workflow_logging_dir() {
        let dir = tempfile::tempdir().unwrap();
        let service_dir = dir.path().join("service");
        let workflow_path = dir.path().join("WORKFLOW.md");
        write_workflow_with_logging_dir(&workflow_path, "work", "workflow-logs");
        let paths = ServicePaths::from_dir(service_dir);

        register_workflow_in_registry(&paths, Some(workflow_path)).unwrap();

        assert_eq!(
            daemon_log_dir(&paths),
            fs::canonicalize(dir.path()).unwrap().join("workflow-logs")
        );
    }

    #[test]
    fn daemon_log_dir_falls_back_to_service_logs_without_registered_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ServicePaths::from_dir(dir.path().join("service"));

        assert_eq!(daemon_log_dir(&paths), paths.service_dir.join("logs"));
    }

    #[test]
    fn registered_workflow_persists_runtime_env_for_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let service_dir = dir.path().join("service");
        let workflow_path = dir.path().join("WORKFLOW.md");
        let env_key = unique_env_key("REGISTRATION");
        write_workflow_with_api_key(&workflow_path, "work", &format!("${env_key}"));
        let paths = ServicePaths::from_dir(service_dir);
        let registration_env = HashMap::from([
            (env_key.clone(), "from_registration".to_string()),
            ("UNRELATED_SECRET".to_string(), "do-not-persist".to_string()),
        ]);

        register_workflow_in_registry_with_env(
            &paths,
            Some(workflow_path.clone()),
            registration_env,
        )
        .unwrap();
        fs::write(dir.path().join(".env"), format!("{env_key}=from_dotenv\n")).unwrap();

        let registry = read_registry(&paths.registry_path).unwrap();
        let workflow = &registry.workflows[0];
        assert_eq!(
            workflow.registration_env.get(&env_key).map(String::as_str),
            Some("from_registration")
        );
        assert!(!workflow.registration_env.contains_key("UNRELATED_SECRET"));
        let runtime_env = workflow_runtime_env(workflow).unwrap();
        assert_eq!(
            runtime_env.get(&env_key).map(String::as_str),
            Some("from_registration")
        );
        let loaded = load_effective_workflow_with_env(Some(workflow_path), &runtime_env).unwrap();
        assert_eq!(loaded.config.tracker.api_key, "from_registration");
    }

    #[test]
    fn registration_env_captures_effective_dotenv_refs() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");
        let env_key = unique_env_key("DOTENV_CAPTURE");
        write_workflow_with_api_key(&workflow_path, "work", &format!("${env_key}"));
        fs::write(dir.path().join(".env"), format!("{env_key}=from_dotenv\n")).unwrap();
        let paths = ServicePaths::from_dir(dir.path().join("service"));

        register_workflow_in_registry_with_env(&paths, Some(workflow_path.clone()), HashMap::new())
            .unwrap();

        let registry = read_registry(&paths.registry_path).unwrap();
        let workflow = &registry.workflows[0];
        assert_eq!(
            workflow.registration_env.get(&env_key).map(String::as_str),
            Some("from_dotenv")
        );
        let runtime_env = workflow_runtime_env_with_process_env(
            workflow,
            HashMap::from([(env_key.clone(), "stale_daemon".to_string())]),
        )
        .unwrap();
        assert_eq!(
            runtime_env.get(&env_key).map(String::as_str),
            Some("from_dotenv")
        );
        let loaded = load_effective_workflow_with_env(Some(workflow_path), &runtime_env).unwrap();
        assert_eq!(loaded.config.tracker.api_key, "from_dotenv");
    }

    #[test]
    fn registration_env_captures_embedded_string_refs() {
        let config = serde_yaml::from_str::<YamlValue>(
            r#"
hooks:
  before_run: echo $HOOK_TOKEN
codex:
  command: ${CODEX_BIN} app-server
workspace:
  root: $WORKSPACE_ROOT/nested
"#,
        )
        .unwrap();
        let config = config.as_mapping().unwrap();
        let captured = capture_registration_env(
            config,
            &HashMap::from([
                ("HOOK_TOKEN".to_string(), "hook-secret".to_string()),
                ("CODEX_BIN".to_string(), "codex-custom".to_string()),
                ("WORKSPACE_ROOT".to_string(), "/tmp/workspaces".to_string()),
                ("UNRELATED_SECRET".to_string(), "do-not-persist".to_string()),
            ]),
        );

        assert_eq!(
            captured.get("HOOK_TOKEN").map(String::as_str),
            Some("hook-secret")
        );
        assert_eq!(
            captured.get("CODEX_BIN").map(String::as_str),
            Some("codex-custom")
        );
        assert_eq!(
            captured.get("WORKSPACE_ROOT").map(String::as_str),
            Some("/tmp/workspaces")
        );
        assert!(!captured.contains_key("UNRELATED_SECRET"));
    }

    #[test]
    fn re_registering_workflow_updates_registration_env_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let workflow_path = dir.path().join("WORKFLOW.md");
        let env_key = unique_env_key("REREGISTRATION");
        write_workflow_with_api_key(&workflow_path, "work", &format!("${env_key}"));
        let paths = ServicePaths::from_dir(dir.path().join("service"));

        register_workflow_in_registry_with_env(
            &paths,
            Some(workflow_path.clone()),
            HashMap::from([(env_key.clone(), "old".to_string())]),
        )
        .unwrap();
        let first = read_registry(&paths.registry_path).unwrap().workflows[0].clone();

        register_workflow_in_registry_with_env(
            &paths,
            Some(workflow_path),
            HashMap::from([(env_key.clone(), "new".to_string())]),
        )
        .unwrap();

        let registry = read_registry(&paths.registry_path).unwrap();
        assert_eq!(registry.workflows.len(), 1);
        let refreshed = &registry.workflows[0];
        assert_eq!(
            refreshed.registration_env.get(&env_key).map(String::as_str),
            Some("new")
        );
        assert_ne!(first, *refreshed);
    }

    #[test]
    fn registered_workflow_runtime_equivalence_ignores_registration_timestamp() {
        let mut left = RegisteredWorkflow {
            workflow_path: PathBuf::from("/tmp/workflow/WORKFLOW.md"),
            cwd: PathBuf::from("/tmp/workflow"),
            registration_env: HashMap::from([("LINEAR_API_KEY".to_string(), "old".to_string())]),
            registered_at_unix: 1,
        };
        let mut right = left.clone();
        right.registered_at_unix = 2;

        assert!(left.runtime_equivalent(&right));

        right
            .registration_env
            .insert("LINEAR_API_KEY".to_string(), "new".to_string());
        assert!(!left.runtime_equivalent(&right));

        left.registration_env = right.registration_env.clone();
        right.cwd = PathBuf::from("/tmp/other");
        assert!(!left.runtime_equivalent(&right));
    }

    #[test]
    fn concurrent_workflow_registration_keeps_all_entries() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ServicePaths::from_dir(dir.path().join("service"));
        let workflow_paths: Vec<_> = (0..4)
            .map(|index| {
                let workflow_path = dir.path().join(format!("WORKFLOW-{index}.md"));
                write_workflow(&workflow_path, &format!("work-{index}"));
                workflow_path
            })
            .collect();

        let handles: Vec<_> = workflow_paths
            .iter()
            .cloned()
            .map(|workflow_path| {
                let paths = paths.clone();
                thread::spawn(move || {
                    register_workflow_in_registry(&paths, Some(workflow_path)).unwrap();
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let registry = read_registry(&paths.registry_path).unwrap();
        let actual: Vec<_> = registry
            .workflows
            .iter()
            .map(|workflow| workflow.workflow_path.clone())
            .collect();
        let mut expected: Vec<_> = workflow_paths
            .iter()
            .map(|workflow_path| fs::canonicalize(workflow_path).unwrap())
            .collect();
        expected.sort();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn reconcile_registry_read_failure_is_non_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let registry_path = dir.path().join("service").join("workflows.json");
        fs::create_dir_all(registry_path.parent().unwrap()).unwrap();
        fs::write(&registry_path, "{not-json").unwrap();
        let runtimes: Arc<Mutex<BTreeMap<PathBuf, WorkflowRuntime>>> =
            Arc::new(Mutex::new(BTreeMap::new()));

        reconcile_registered_workflows(&registry_path, Arc::clone(&runtimes))
            .await
            .unwrap();

        assert!(runtimes.lock().await.is_empty());
    }

    #[test]
    fn uninstall_removes_service_state_and_registry() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ServicePaths::from_dir(dir.path().join("service"));
        let target = ServiceTarget {
            paths: paths.clone(),
            port: None,
            bind_address: None,
            legacy_workdir: dir.path().to_path_buf(),
        };
        let state = service_state_with_port(None);
        write_state(&paths.state_path, &state).unwrap();
        write_registry(
            &paths.registry_path,
            &WorkflowRegistry {
                version: 1,
                workflows: vec![RegisteredWorkflow {
                    workflow_path: dir.path().join("WORKFLOW.md"),
                    cwd: dir.path().to_path_buf(),
                    registration_env: HashMap::new(),
                    registered_at_unix: 1,
                }],
            },
        )
        .unwrap();

        target.uninstall().unwrap();

        assert!(!paths.state_path.exists());
        assert!(!paths.registry_path.exists());
    }

    #[test]
    fn uninstall_removes_registry_without_service_state() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ServicePaths::from_dir(dir.path().join("service"));
        let target = ServiceTarget {
            paths: paths.clone(),
            port: None,
            bind_address: None,
            legacy_workdir: dir.path().to_path_buf(),
        };
        write_registry(
            &paths.registry_path,
            &WorkflowRegistry {
                version: 1,
                workflows: vec![RegisteredWorkflow {
                    workflow_path: dir.path().join("WORKFLOW.md"),
                    cwd: dir.path().to_path_buf(),
                    registration_env: HashMap::new(),
                    registered_at_unix: 1,
                }],
            },
        )
        .unwrap();

        target.uninstall().unwrap();

        assert!(!paths.state_path.exists());
        assert!(!paths.registry_path.exists());
    }

    #[test]
    fn legacy_service_state_paths_find_old_workflow_files() {
        let dir = tempfile::tempdir().unwrap();
        let service_dir = dir.path().join(".vik").join("service");
        fs::create_dir_all(&service_dir).unwrap();
        let legacy_path = service_dir.join("WORKFLOW-123.json");
        fs::write(&legacy_path, "{}").unwrap();
        fs::write(service_dir.join("service.json"), "{}").unwrap();
        fs::write(service_dir.join("workflows.json"), "{}").unwrap();
        fs::write(service_dir.join("service.log"), "").unwrap();

        assert_eq!(
            legacy_service_state_paths_in_workdir(dir.path()).unwrap(),
            vec![legacy_path]
        );
    }

    #[test]
    fn stop_legacy_services_removes_old_stopped_state() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_path = dir
            .path()
            .join(".vik")
            .join("service")
            .join("WORKFLOW-123.json");
        write_legacy_state(
            &legacy_path,
            &LegacyServiceState {
                version: 1,
                workflow_path: dir.path().join("WORKFLOW.md"),
                cwd: dir.path().to_path_buf(),
                pid: None,
                status: StoredStatus::Stopped,
                started_at_unix: Some(1),
                stopped_at_unix: Some(2),
                log_path: dir
                    .path()
                    .join(".vik")
                    .join("service")
                    .join("WORKFLOW-123.log"),
                port: None,
                command: vec![],
            },
        )
        .unwrap();

        let stopped = stop_legacy_services_in_workdir(dir.path(), true).unwrap();

        assert_eq!(stopped, 1);
        assert!(!legacy_path.exists());
    }

    #[test]
    fn command_match_requires_service_dir() {
        let service_dir = PathBuf::from("/tmp/vik/.vik/service");

        assert!(command_mentions_service_dir(
            "vik daemon --service-dir /tmp/vik/.vik/service --port 3000",
            &service_dir
        ));
        assert!(!command_mentions_service_dir(
            "vik daemon --service-dir /tmp/other/.vik/service",
            &service_dir
        ));
    }

    #[test]
    fn workflow_env_from_dir_reads_dotenv_without_mutating_process_env() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        fs::create_dir_all(&nested).unwrap();
        let key = unique_env_key("ISOLATED");
        fs::write(dir.path().join(".env"), format!("{key}=from_dotenv\n")).unwrap();

        let env_map = workflow_env_from_dir(&nested).unwrap();

        assert_eq!(env_map.get(&key).map(String::as_str), Some("from_dotenv"));
        assert!(env::var(key).is_err());
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
            version: 2,
            service_dir: PathBuf::from("/tmp/vik/.vik/service"),
            registry_path: PathBuf::from("/tmp/vik/.vik/service/workflows.json"),
            cwd: PathBuf::from("/tmp/vik"),
            pid: Some(999_999),
            status: StoredStatus::Stopped,
            started_at_unix: Some(1),
            stopped_at_unix: Some(2),
            log_path: PathBuf::from("/tmp/vik/service.log"),
            port: None,
            bind_address: None,
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
            version: 2,
            service_dir: PathBuf::from("/tmp/vik/.vik/service"),
            registry_path: PathBuf::from("/tmp/vik/.vik/service/workflows.json"),
            cwd,
            pid: None,
            status: StoredStatus::Stopped,
            started_at_unix: Some(1),
            stopped_at_unix: Some(2),
            log_path: PathBuf::from("/tmp/vik/service.log"),
            port,
            bind_address: None,
            command: vec![],
        }
    }

    fn service_target() -> ServiceTarget {
        ServiceTarget {
            paths: ServicePaths::from_dir(PathBuf::from("/tmp/vik/.vik/service")),
            port: None,
            bind_address: None,
            legacy_workdir: PathBuf::from("/tmp/vik"),
        }
    }

    fn write_workflow(path: &Path, workspace_root: &str) {
        write_workflow_with_api_key(path, workspace_root, "token");
    }

    fn write_workflow_with_api_key(path: &Path, workspace_root: &str, api_key: &str) {
        fs::write(
            path,
            format!(
                "---\ntracker:\n  kind: linear\n  api_key: {api_key}\n  project_slug: proj\nworkspace:\n  root: {workspace_root}\n---\nBody"
            ),
        )
        .unwrap();
    }

    fn write_workflow_with_logging_dir(path: &Path, workspace_root: &str, logging_dir: &str) {
        fs::write(
            path,
            format!(
                "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: proj\nworkspace:\n  root: {workspace_root}\nlogging:\n  dir: {logging_dir}\n---\nBody"
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
