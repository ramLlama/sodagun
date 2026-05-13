use clap::{Parser, Subcommand};

mod commands;
mod context;
mod error;

use commands::git::GitCommand;
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

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Git utilities.
    Git(GitCommand),
}

fn main() {
    let cli = Cli::parse();
    let ctx = Context { output: cli.output };

    match cli.command {
        Commands::Git(cmd) => commands::git::run(ctx, cmd),
    }
}
