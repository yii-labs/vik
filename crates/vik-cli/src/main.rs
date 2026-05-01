use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;
use vik_agent::LocalAgentWorker;
use vik_http::{HttpState, serve};
use vik_orchestrator::Orchestrator;
use vik_tracker::{DEFAULT_LINEAR_ENDPOINT, LinearClient, LinearClientConfig};
use vik_workflow::WorkflowReloader;

#[derive(Debug, Parser)]
#[command(name = "vik", version, about = "Run Vik coding-agent orchestrator")]
struct Args {
    /// Path to WORKFLOW.md. Defaults to ./WORKFLOW.md.
    workflow: Option<PathBuf>,

    /// Enable HTTP status server. Overrides server.port from WORKFLOW.md.
    #[arg(long)]
    port: Option<u16>,

    /// Validate workflow and exit.
    #[arg(long)]
    check: bool,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("vik startup failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    load_dotenv()?;
    init_logging();
    let args = Args::parse();
    let reloader = WorkflowReloader::start(args.workflow.clone())?;
    let loaded = reloader.current().clone();
    loaded.config.validate_for_dispatch()?;
    if args.check {
        println!("workflow valid: {}", loaded.definition.path.display());
        return Ok(());
    }

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

    let port = args
        .port
        .or(loaded.config.server.as_ref().map(|server| server.port));
    if let Some(port) = port {
        let orch_for_state = Arc::clone(&orchestrator);
        let orch_for_issue = Arc::clone(&orchestrator);
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
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

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .init();
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
}
