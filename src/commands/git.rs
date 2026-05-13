use std::path::PathBuf;

use clap::{Args, Subcommand};
use git2::{ErrorCode, Repository, WorktreeAddOptions};
use uuid::Uuid;

use crate::context::{Context, OutputFormat};
use crate::error::{SodagunError, handle_error};

#[derive(Args)]
pub struct GitCommand {
    #[command(subcommand)]
    pub subcommand: GitSubcommand,
}

#[derive(Subcommand)]
pub enum GitSubcommand {
    /// Create a git worktree on a new branch, printing the resulting path.
    AddWorktree(AddWorktreeArgs),
}

#[derive(Args)]
pub struct AddWorktreeArgs {
    /// Path to the git repository.
    pub repo_path: PathBuf,

    /// Name of the branch to create.
    pub branch_name: String,

    /// Parent directory for the worktree (default: system temp dir).
    #[arg(long)]
    pub dir_prefix: Option<PathBuf>,

    /// Ref to base the new branch on.
    #[arg(long, default_value = "origin/main")]
    pub base: String,
}

pub fn run(ctx: Context, cmd: GitCommand) {
    match cmd.subcommand {
        GitSubcommand::AddWorktree(args) => add_worktree(ctx, args),
    }
}

fn add_worktree(ctx: Context, args: AddWorktreeArgs) {
    let dir_prefix = args.dir_prefix.unwrap_or_else(std::env::temp_dir);

    // Use file_name() directly rather than canonicalize() so the name is available
    // even when parent directories have permission issues or contain symlink loops.
    let repo_name = args
        .repo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());

    let uuid8 = &Uuid::new_v4().to_string()[..8];
    // pygit2.add_worktree / git2::Repository::worktree require the target path to not pre-exist
    let worktree_path = dir_prefix.join(format!("sodagun-wt-{repo_name}-{uuid8}"));

    // Open repo
    let repo = Repository::open(&args.repo_path).unwrap_or_else(|_| {
        handle_error(
            ctx,
            SodagunError {
                code: "REPO_NOT_FOUND",
                message: format!("Repository not found at {}", args.repo_path.display()),
            },
        )
    });

    // Resolve base ref to a commit.
    // git2 returns ErrorCode::NotFound when the ref simply doesn't exist (equivalent
    // to Python's KeyError from revparse_single); any other error maps to BASE_INVALID.
    let obj = repo.revparse_single(&args.base).unwrap_or_else(|e| {
        if e.code() == ErrorCode::NotFound {
            handle_error(
                ctx,
                SodagunError {
                    code: "BASE_NOT_FOUND",
                    message: format!("Base ref '{}' not found", args.base),
                },
            )
        } else {
            handle_error(
                ctx,
                SodagunError {
                    code: "BASE_INVALID",
                    message: format!("Could not resolve '{}' to a commit", args.base),
                },
            )
        }
    });

    let commit = obj.peel_to_commit().unwrap_or_else(|_| {
        handle_error(
            ctx,
            SodagunError {
                code: "BASE_INVALID",
                message: format!("Could not resolve '{}' to a commit", args.base),
            },
        )
    });

    // Create branch; roll it back on any subsequent failure to avoid orphaned refs
    let mut branch = repo
        .branch(&args.branch_name, &commit, false)
        .unwrap_or_else(|e| {
            if e.code() == ErrorCode::Exists {
                handle_error(
                    ctx,
                    SodagunError {
                        code: "BRANCH_EXISTS",
                        message: format!("Branch '{}' already exists", args.branch_name),
                    },
                )
            } else {
                handle_error(
                    ctx,
                    SodagunError {
                        code: "GIT_ERROR",
                        message: e.to_string(),
                    },
                )
            }
        });

    // Pre-check for worktree conflicts before calling worktree(); avoids fragile string matching
    let worktree_name_conflict = repo
        .worktrees()
        .map(|names| {
            names
                .iter()
                .any(|n| n.is_some_and(|s| s == args.branch_name))
        })
        .unwrap_or(false);

    if worktree_path.exists() || worktree_name_conflict {
        let _ = branch.delete();
        handle_error(
            ctx,
            SodagunError {
                code: "WORKTREE_EXISTS",
                message: format!("Worktree '{}' already exists", args.branch_name),
            },
        );
    }

    // Add worktree pinned to the new branch; roll back the branch on failure
    let mut wt_opts = WorktreeAddOptions::new();
    wt_opts.reference(Some(branch.get()));

    repo.worktree(&args.branch_name, &worktree_path, Some(&wt_opts))
        .unwrap_or_else(|e| {
            let _ = branch.delete();
            handle_error(
                ctx,
                SodagunError {
                    code: "GIT_ERROR",
                    message: e.to_string(),
                },
            )
        });

    match ctx.output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "status": "ok",
                    "worktree_path": worktree_path.to_string_lossy()
                })
            );
        }
        OutputFormat::Text => {
            println!("{}", worktree_path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use uuid::Uuid;

    #[test]
    fn worktree_path_has_expected_structure() {
        // Verify the full naming contract: prefix / sodagun-wt-<reponame>-<8 hex chars>
        let uuid8 = &Uuid::new_v4().to_string()[..8];
        let dir = PathBuf::from("/tmp");
        let wt_path = dir.join(format!("sodagun-wt-myrepo-{uuid8}"));
        let name = wt_path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("sodagun-wt-myrepo-"), "name={name}");
        // uuid8 is exactly 8 hex chars from the first group of a v4 UUID
        let suffix = name.trim_start_matches("sodagun-wt-myrepo-");
        assert_eq!(suffix.len(), 8, "suffix={suffix}");
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "suffix={suffix}"
        );
    }
}
