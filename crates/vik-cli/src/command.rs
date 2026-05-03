use std::error::Error;
use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "vik",
    version,
    about = "Run Vik coding-agent orchestrator",
    subcommand_required = true,
    arg_required_else_help = true
)]
pub(crate) struct Args {
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
    Check(crate::check::CheckArgs),
    /// Manage Vik as a detached local service.
    Service(crate::service::ServiceArgs),
}

pub(crate) async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    match args.command {
        Command::Start(args) => {
            load_dotenv()?;
            crate::start::run(args.workflow, args.port, args.bind_address).await
        }
        Command::Check(args) => {
            load_dotenv()?;
            crate::check::run(args.workflow)
        }
        Command::Service(args) => crate::service::run(args).await,
    }
}

fn load_dotenv() -> Result<(), Box<dyn Error>> {
    crate::env::load_dotenv()
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;

    use clap::CommandFactory;

    use super::*;

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
}
