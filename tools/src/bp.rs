use clap::{Parser, Subcommand, ValueEnum};

mod app_echo;
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
    /// Send one payload via BPA gRPC and wait for an echo response
    AppEcho(app_echo::Command),
    /// Send one payload via BPA gRPC and exit (for forward/store demos)
    AppSend(app_send::Command),
}

fn main() {
    // Match on the parsed subcommand and call the appropriate handler function.
    // This is the core of the dispatch logic.
    match Cli::parse().command {
        Commands::Ping(args) => args.exec(),
        Commands::AppEcho(args) => app_echo::exec(args),
        Commands::AppSend(args) => app_send::exec(args),
    }
}
