use std::error::Error;
use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Args as ClapArgs, Subcommand};

mod manager;

const DEFAULT_SERVICE_PORT: u16 = 7788;

#[derive(Debug, ClapArgs)]
pub(crate) struct ServiceArgs {
    #[command(subcommand)]
    command: ServiceCommand,
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Start Vik in the background.
    Start(RunArgs),
    /// Stop a running Vik service.
    Stop,
    /// Stop then start Vik in the background.
    Restart(RunArgs),
    /// Print current service status.
    Status,
    /// Print recent service logs.
    Logs(LogsArgs),
    /// Remove service state and stop Vik if it is running.
    Uninstall,
}

#[derive(Debug, Clone, Copy, ClapArgs)]
pub(crate) struct RunArgs {
    /// HTTP status server port. Overrides server.port from WORKFLOW.md.
    #[arg(long, short, default_value_t = DEFAULT_SERVICE_PORT)]
    pub(crate) port: u16,

    /// HTTP status server bind address. Defaults to 127.0.0.1.
    #[arg(
        long,
        alias = "bind-address",
        value_name = "ADDR",
        default_value = "127.0.0.1"
    )]
    pub(crate) host: IpAddr,
}

#[derive(Debug, ClapArgs)]
pub(crate) struct StartArgs {
    #[command(flatten)]
    pub(crate) run_args: RunArgs,

    /// Start Vik as a detached background service. Defaults to false.
    #[arg(long, short, default_value_t = false)]
    pub(crate) detached: bool,
}

impl From<RunArgs> for StartArgs {
    fn from(run_args: RunArgs) -> Self {
        Self {
            run_args,
            detached: false,
        }
    }
}

#[derive(Debug, Clone, ClapArgs)]
struct LogsArgs {
    /// Number of recent lines to print.
    #[arg(long, short, default_value_t = 100)]
    lines: usize,

    /// Continue printing appended log output.
    #[arg(long, short, default_value_t = false)]
    follow: bool,
}

pub(crate) async fn run(
    workflow: Option<PathBuf>,
    args: ServiceArgs,
) -> Result<(), Box<dyn Error>> {
    let manager = manager::ServiceManager::new(workflow)?;
    match args.command {
        ServiceCommand::Start(run_args) => {
            manager
                .start(StartArgs {
                    run_args,
                    detached: true,
                })
                .await?;
        }
        ServiceCommand::Uninstall => manager.uninstall()?,
        ServiceCommand::Status => manager.status()?,
        ServiceCommand::Logs(args) => manager.print_logs(args)?,
        ServiceCommand::Stop => manager.stop()?,
        ServiceCommand::Restart(args) => manager.restart(args).await?,
    }
    Ok(())
}

pub(crate) async fn start(
    workflow: Option<PathBuf>,
    args: StartArgs,
) -> Result<(), Box<dyn Error>> {
    let manager = manager::ServiceManager::new(workflow)?;
    manager.start(args).await
}
