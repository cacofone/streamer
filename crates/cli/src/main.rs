//! Command line interface of Cacofone Streamer.

mod discover;
mod output;

use std::io::{self, Write};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use streamer_core::ssdp::SsdpError;
use streamer_core::ssdp::discovery::Tuning;
use tracing::Level;

use discover::{CliError, DiscoverArgs};

/// Plays music from your home media server to speakers on your network.
#[derive(Debug, Parser)]
#[command(name = "streamer", version, about)]
struct Cli {
    /// Show more detail on stderr (-v for info, -vv for debug, -vvv for trace)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Find media servers and renderers on the local network
    Discover(DiscoverArgs),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let result = match cli.command {
        Command::Discover(args) => {
            let stop = async {
                // if the signal handler cannot be installed, just never stop early
                if tokio::signal::ctrl_c().await.is_err() {
                    std::future::pending::<()>().await;
                }
            };
            discover::run(
                &args,
                Tuning::default(),
                &mut io::stdout(),
                &mut io::stderr(),
                stop,
            )
            .await
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        // the reader went away (for example `| head`): not an error
        Err(CliError::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            let mut stderr = io::stderr();
            let _ = writeln!(stderr, "error: {error}");
            if let CliError::Discovery(SsdpError::NoUsableInterface) = error {
                let _ = writeln!(
                    stderr,
                    "hint: check your network connection, or name an interface with --interface"
                );
            }
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(verbose: u8) {
    let level = match verbose {
        0 => Level::WARN,
        1 => Level::INFO,
        2 => Level::DEBUG,
        _ => Level::TRACE,
    };
    // logs go to stderr so that stdout stays clean for scripts
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(io::stderr)
        .with_target(false)
        .try_init();
}
