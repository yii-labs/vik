mod agent;
mod cli;
mod config;
mod context;
mod daemon;
mod hooks;
mod logging;
mod orchestrator;
mod server;
mod session;
mod shell;
mod template;
mod utils;
mod workflow;
mod workspace;

use std::process::ExitCode;

fn main() -> ExitCode {
  cli::run()
}
