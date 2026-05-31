use clap::{Parser, Subcommand};

mod commands;
mod config;
mod context;
mod error;
mod workspace;

use commands::git::GitCommand;
use commands::sandbox::SandboxCommand;
use commands::snapshot::SnapshotCommand;
use context::{Context, OutputFormat};

#[derive(Parser)]
#[command(
    name = "sodagun",
    version = env!("CARGO_PKG_VERSION"),
    about = "sodagun CLI",
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    /// Output format.
    #[arg(long, default_value = "text", value_enum)]
    output: OutputFormat,

    /// Suppress progress output (setup script logs, etc.).
    #[arg(long)]
    quiet: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Git utilities.
    Git(GitCommand),
    /// Sandbox utilities.
    Sandbox(SandboxCommand),
    /// Snapshot utilities.
    Snapshot(SnapshotCommand),
}

fn main() {
    let cli = Cli::parse();
    let ctx = Context {
        output: cli.output,
        quiet: cli.quiet,
    };

    match cli.command {
        Commands::Git(cmd) => commands::git::run(ctx, cmd),
        Commands::Sandbox(cmd) => commands::sandbox::run(ctx, cmd),
        Commands::Snapshot(cmd) => commands::snapshot::run(ctx, cmd),
    }
}
