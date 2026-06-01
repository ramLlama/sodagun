use std::path::PathBuf;

use clap::{Parser, Subcommand};
use colored::Colorize;

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

    /// Project directory override. Defaults to the nearest ancestor (including CWD)
    /// that contains a sodagun.toml or .git directory.
    #[arg(long)]
    project_dir: Option<PathBuf>,

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

/// Walk up from CWD to find the project root, using `sodagun.toml` or `.git/`
/// as markers. Warns (unless quiet) if `sodagun.toml` is not co-located with `.git`.
fn find_project_dir(override_dir: Option<PathBuf>, quiet: bool) -> PathBuf {
    if let Some(dir) = override_dir {
        return dir;
    }

    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!(
            "{} cannot determine working directory: {e}",
            "error:".red().bold()
        );
        std::process::exit(1);
    });

    let mut project_dir: Option<PathBuf> = None;
    let mut git_root: Option<PathBuf> = None;
    let mut toml_dir: Option<PathBuf> = None;

    let mut dir: &std::path::Path = &cwd;
    loop {
        let has_toml = dir.join("sodagun.toml").exists();
        let has_git = dir.join(".git").exists();

        if project_dir.is_none() && (has_toml || has_git) {
            project_dir = Some(dir.to_path_buf());
        }
        if has_toml && toml_dir.is_none() {
            toml_dir = Some(dir.to_path_buf());
        }
        if has_git && git_root.is_none() {
            git_root = Some(dir.to_path_buf());
        }

        // Stop once we have everything we need for the warning check.
        if project_dir.is_some() && toml_dir.is_some() && git_root.is_some() {
            break;
        }

        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }

    let project_dir = project_dir.unwrap_or_else(|| {
        eprintln!(
            "{} no sodagun.toml or .git directory found; use --project-dir to specify the project root",
            "error:".red().bold()
        );
        std::process::exit(1);
    });

    // Warn if sodagun.toml is found but not co-located with .git (not at the project root).
    if !quiet
        && let (Some(td), Some(gd)) = (&toml_dir, &git_root)
        && td != gd
    {
        eprintln!(
            "{} sodagun.toml should be at the project root ({}), not {}",
            "warning:".yellow().bold(),
            gd.display(),
            td.display()
        );
    }

    project_dir
}

fn main() {
    let cli = Cli::parse();
    let ctx = Context {
        output: cli.output,
        quiet: cli.quiet,
    };
    let project_dir = find_project_dir(cli.project_dir, cli.quiet);

    match cli.command {
        Commands::Git(cmd) => commands::git::run(ctx, cmd, project_dir),
        Commands::Sandbox(cmd) => commands::sandbox::run(ctx, cmd),
        Commands::Snapshot(cmd) => commands::snapshot::run(ctx, cmd, project_dir),
    }
}
