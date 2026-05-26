use clap::{Parser, Subcommand, ValueEnum};

use crate::app_send::exec;

mod app_send;
mod ping;

/// Bundle Protocol diagnostic and testing tools.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Send ping bundles and measure round-trip time
    Ping(ping::Command),
    AppSend(app_send::Command),
}

fn main() {
    // Match on the parsed subcommand and call the appropriate handler function.
    // This is the core of the dispatch logic.
    match Cli::parse().command {
        Commands::Ping(args) => args.exec(),
        Commands::AppSend(command) => exec(command),
    }
}
