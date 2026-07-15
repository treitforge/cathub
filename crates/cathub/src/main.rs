//! Binary entry point for the `CatHub` daemon.

use std::process::ExitCode;

use clap::Parser;

use cathub::{run, Cli};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let _guard = cathub::init_logging();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(error = %err, "cathub exited with error");
            eprintln!("cathub: {err}");
            ExitCode::FAILURE
        }
    }
}
