use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io};

mod check;
mod service;

#[cfg(test)]
use clap::CommandFactory;
use clap::{Parser, Subcommand};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use vik_agent::LocalAgentWorker;
use vik_http::{HttpState, serve};
use vik_orchestrator::Orchestrator;
use vik_tracker::{DEFAULT_LINEAR_ENDPOINT, LinearClient, LinearClientConfig};
use vik_workflow::WorkflowReloader;

#[derive(Debug, Parser)]
#[command(
    name = "vik",
    version,
    about = "Run Vik coding-agent orchestrator",
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Args)]
struct StartArgs {
    /// Path to WORKFLOW.md. Defaults to ./WORKFLOW.md.
    workflow: Option<PathBuf>,

    /// Enable HTTP status server. Overrides server.port from WORKFLOW.md.
    #[arg(long)]
    port: Option<u16>,

    /// HTTP status server bind address. Defaults to 127.0.0.1.
    #[arg(long, alias = "host", value_name = "ADDR")]
    bind_address: Option<IpAddr>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start Vik coding-agent orchestration.
    Start(StartArgs),
    /// Validate workflow and exit.
    Check(check::CheckArgs),
    /// Manage Vik as a detached local service.
    Service(service::ServiceArgs),
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("vik startup failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    match args.command {
        Command::Start(args) => {
            load_dotenv()?;
            run_daemon(args.workflow, args.port, args.bind_address).await
        }
        Command::Check(args) => {
            load_dotenv()?;
            check::run(args.workflow)
        }
        Command::Service(args) => service::run(args).await,
    }
}

async fn run_daemon(
    workflow: Option<PathBuf>,
    port: Option<u16>,
    bind_address: Option<IpAddr>,
) -> Result<(), Box<dyn std::error::Error>> {
    let reloader = WorkflowReloader::start(workflow)?;
    let loaded = reloader.current().clone();
    loaded.config.validate_for_dispatch()?;

    let log_dir = loaded.config.logging.dir.clone();
    let _log_guard = init_logging(&log_dir)?;
    tracing::info!(log_dir=%log_dir.display(), "logging outcome=started");

    let tracker_config = LinearClientConfig::new(
        if loaded.config.tracker.endpoint.is_empty() {
            DEFAULT_LINEAR_ENDPOINT
        } else {
            &loaded.config.tracker.endpoint
        },
        &loaded.config.tracker.api_key,
        &loaded.config.tracker.project_slug,
        loaded.config.tracker.active_states.clone(),
    );
    let tracker = Arc::new(LinearClient::new(tracker_config)?);
    let worker = Arc::new(LocalAgentWorker::new(Arc::clone(&tracker)));
    let orchestrator = Arc::new(Orchestrator::new(Arc::clone(&tracker), worker, reloader));

    let port = port.or(loaded.config.server.as_ref().map(|server| server.port));
    if let Some(port) = port {
        let orch_for_state = Arc::clone(&orchestrator);
        let orch_for_issue = Arc::clone(&orchestrator);
        let addr = http_addr(bind_address, port);
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

fn load_dotenv() -> Result<(), Box<dyn std::error::Error>> {
    match dotenvy::dotenv() {
        Ok(_) => Ok(()),
        Err(dotenvy::Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("failed to load .env: {err}").into()),
    }
}

#[cfg(test)]
fn load_dotenv_path(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    match dotenvy::from_path(path) {
        Ok(_) => Ok(()),
        Err(err) => Err(format!("failed to load {}: {err}", path.display()).into()),
    }
}

fn init_logging(log_dir: &Path) -> Result<WorkerGuard, Box<dyn std::error::Error>> {
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

fn http_addr(host: Option<IpAddr>, port: u16) -> SocketAddr {
    SocketAddr::new(host.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)), port)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use tempfile::tempdir;

    use super::*;

    fn unique_env_key(suffix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("VIK_TEST_DOTENV_{nanos}_{suffix}")
    }

    #[test]
    fn load_dotenv_path_sets_missing_env_var() {
        let dir = tempdir().unwrap();
        let key = unique_env_key("SET");
        let env_path = dir.path().join(".env");
        fs::write(&env_path, format!("{key}=from_dotenv\n")).unwrap();

        load_dotenv_path(&env_path).unwrap();

        assert_eq!(std::env::var(key).unwrap(), "from_dotenv");
    }

    #[test]
    fn load_dotenv_path_does_not_override_existing_env_var() {
        let dir = tempdir().unwrap();
        let key = unique_env_key("PRESERVE");
        let first_path = dir.path().join(".env.first");
        let second_path = dir.path().join(".env.second");
        fs::write(&first_path, format!("{key}=first\n")).unwrap();
        fs::write(&second_path, format!("{key}=second\n")).unwrap();

        load_dotenv_path(&first_path).unwrap();
        load_dotenv_path(&second_path).unwrap();

        assert_eq!(std::env::var(key).unwrap(), "first");
    }

    #[test]
    fn load_dotenv_path_reports_parse_errors() {
        let dir = tempdir().unwrap();
        let env_path = dir.path().join(".env");
        fs::write(&env_path, "BROKEN=\"unterminated\n").unwrap();

        let err = load_dotenv_path(&env_path).unwrap_err().to_string();

        assert!(err.contains("failed to load"));
    }

    #[test]
    fn check_subcommand_accepts_workflow_path() {
        let args = Args::try_parse_from(["vik", "check", "./custom.md"]).unwrap();

        match args.command {
            Command::Check(check_args) => {
                assert_eq!(check_args.workflow, Some(PathBuf::from("./custom.md")));
            }
            other => panic!("expected check subcommand, got {other:?}"),
        }
    }

    #[test]
    fn check_subcommand_allows_default_workflow_path() {
        let args = Args::try_parse_from(["vik", "check"]).unwrap();

        match args.command {
            Command::Check(check_args) => assert_eq!(check_args.workflow, None),
            other => panic!("expected check subcommand, got {other:?}"),
        }
    }

    #[test]
    fn legacy_check_flag_is_rejected() {
        let err = Args::try_parse_from(["vik", "./WORKFLOW.md", "--check"]).unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn root_help_shows_check_command_not_legacy_flag() {
        let help = Args::command().render_help().to_string();

        assert!(help.contains("start"));
        assert!(help.contains("check"));
        assert!(help.contains("Start Vik coding-agent orchestration"));
        assert!(help.contains("Validate workflow and exit"));
        assert!(!help.contains("--check"));
    }

    #[test]
    fn start_command_accepts_workflow_and_daemon_flags() {
        let args = Args::try_parse_from([
            "vik",
            "start",
            "WORKFLOW.md",
            "--port",
            "3000",
            "--bind-address",
            "0.0.0.0",
        ])
        .unwrap();

        match args.command {
            Command::Start(args) => {
                assert_eq!(args.workflow, Some(PathBuf::from("WORKFLOW.md")));
                assert_eq!(args.port, Some(3000));
                assert_eq!(args.bind_address, Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
            }
            other => panic!("expected start command, got {other:?}"),
        }
    }

    #[test]
    fn implicit_workflow_arg_is_rejected() {
        let err = Args::try_parse_from(["vik", "WORKFLOW.md"]).unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn daemon_flags_are_scoped_to_start_command() {
        let err = Args::try_parse_from(["vik", "--port", "3000", "service", "status"])
            .unwrap_err()
            .kind();

        assert_eq!(err, clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn http_addr_defaults_to_localhost() {
        assert_eq!(
            http_addr(None, 3000),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3000)
        );
    }

    #[test]
    fn http_addr_uses_explicit_host() {
        assert_eq!(
            http_addr(Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)), 3000),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 3000)
        );
    }
}
