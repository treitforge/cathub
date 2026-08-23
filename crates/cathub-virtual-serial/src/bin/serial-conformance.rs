//! Windows serial API conformance command.

use std::net::SocketAddr;
use std::path::PathBuf;

use cathub_virtual_serial::conformance::{profiles, run, run_with_tcp_peer, ConformanceError};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "serial-conformance",
    version,
    about = "Test a virtual serial pair with the Windows serial API"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List the supported application profiles as JSON.
    Profiles,
    /// Run one profile against an isolated virtual serial pair.
    Run {
        /// Application-facing COM port.
        #[arg(long)]
        application_port: String,
        /// Paired COM port that the harness uses as the transport peer.
        #[arg(
            long,
            required_unless_present = "peer_tcp",
            conflicts_with = "peer_tcp"
        )]
        peer_port: Option<String>,
        /// TCP bridge to a CatHub-managed endpoint's private transport peer.
        #[arg(
            long,
            required_unless_present = "peer_port",
            conflicts_with = "peer_port"
        )]
        peer_tcp: Option<SocketAddr>,
        /// Stable profile name from the profiles command.
        #[arg(long)]
        profile: String,
        /// Optional JSON report path.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

fn main() -> Result<(), ConformanceError> {
    match Cli::parse().command {
        Command::Profiles => {
            println!("{}", serde_json::to_string_pretty(profiles())?);
            Ok(())
        }
        Command::Run {
            application_port,
            peer_port,
            peer_tcp,
            profile,
            output,
        } => {
            let report = match (peer_port, peer_tcp) {
                (Some(peer_port), None) => run(&profile, &application_port, &peer_port)?,
                (None, Some(peer_address)) => {
                    run_with_tcp_peer(&profile, &application_port, peer_address)?
                }
                _ => unreachable!("clap requires exactly one peer transport"),
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
            if let Some(path) = output {
                report.write_json(&path)?;
            }
            if report.required_cases_pass() {
                Ok(())
            } else {
                Err(ConformanceError::InvalidResult(
                    "one or more required conformance cases failed".to_string(),
                ))
            }
        }
    }
}
