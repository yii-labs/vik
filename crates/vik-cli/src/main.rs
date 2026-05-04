use clap::Parser;

mod check;
mod command;
mod env;
mod service;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("vik startup failed: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = command::Args::parse();
    command::run(args).await
}
