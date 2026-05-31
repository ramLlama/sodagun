use std::path::PathBuf;

use clap::{Args, Subcommand};
use git2::{ErrorCode, Repository, WorktreeAddOptions};
use uuid::Uuid;

use crate::context::{Context, OutputFormat};
use crate::error::{SodagunError, handle_error};
use crate::workspace::WorkspaceMetadata;

#[derive(Args)]
pub struct GitCommand {
    #[command(subcommand)]
    pub subcommand: GitSubcommand,
}

#[derive(Subcommand)]
pub enum GitSubcommand {
    /// Create a git worktree on a new branch, printing the resulting rootdir path.
    AddWorktree(AddWorktreeArgs),
}

#[derive(Args)]
pub struct AddWorktreeArgs {
    /// Path to the git repository.
    pub repo_path: PathBuf,

    /// Name of the branch to create.
    pub branch_name: String,

    /// Parent directory for the workspace rootdir (default: system temp dir).
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

    // Canonicalize first so `.` resolves to the real directory name. Fall back to
    // file_name() on the raw path (handles permission errors / symlink loops), and
    // finally to a plain "repo" sentinel if neither yields a usable component.
    let repo_name = args
        .repo_path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .or_else(|| {
            args.repo_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "repo".to_string());

    let uuid8 = &Uuid::new_v4().to_string()[..8];
    // `_` is the structural separator in the rootdir name, so strip it from components
    // by collapsing `/` and `_` in repo/branch names to `-`. Best-effort: components
    // still look clean while `_` stays unambiguous as a delimiter.
    let sanitized_repo = repo_name.replace('_', "-");
    let sanitized_branch = args.branch_name.replace(['/', '_'], "-");
    // Convention: sodagun_{repo}_{branch}_{uuid8} — same name is reused for the sandbox.
    let rootdir = dir_prefix.join(format!(
        "sodagun_{sanitized_repo}_{sanitized_branch}_{uuid8}"
    ));
    let worktree_path = rootdir.join(&sanitized_branch);

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

    // Pre-check for worktree conflicts before calling worktree(); avoids fragile string matching.
    // The worktree name stored in .git/worktrees/ is the sanitized branch (slashes replaced),
    // since git itself cannot create nested directories there.
    let worktree_name_conflict = repo
        .worktrees()
        .map(|names| {
            names
                .iter()
                .any(|n| n.is_some_and(|s| s == sanitized_branch))
        })
        .unwrap_or(false);

    if rootdir.exists() || worktree_path.exists() || worktree_name_conflict {
        let _ = branch.delete();
        handle_error(
            ctx,
            SodagunError {
                code: "WORKTREE_EXISTS",
                message: format!("Worktree '{}' already exists", args.branch_name),
            },
        );
    }

    // Create rootdir to hold the worktree and sodagun.json
    std::fs::create_dir(&rootdir).unwrap_or_else(|e| {
        let _ = branch.delete();
        handle_error(
            ctx,
            SodagunError {
                code: "GIT_ERROR",
                message: format!("failed to create workspace rootdir: {e}"),
            },
        )
    });

    // Add worktree pinned to the new branch; roll back the branch and rootdir on failure.
    // Use sanitized_branch as the worktree name so git doesn't try to nest directories
    // (git stores worktree metadata under .git/worktrees/<name> and rejects slashes).
    let mut wt_opts = WorktreeAddOptions::new();
    wt_opts.reference(Some(branch.get()));

    repo.worktree(&sanitized_branch, &worktree_path, Some(&wt_opts))
        .unwrap_or_else(|e| {
            let _ = branch.delete();
            let _ = std::fs::remove_dir_all(&rootdir);
            handle_error(
                ctx,
                SodagunError {
                    code: "GIT_ERROR",
                    message: e.to_string(),
                },
            )
        });

    // Write workspace metadata; roll back everything on failure
    let repo_path = args
        .repo_path
        .canonicalize()
        .unwrap_or_else(|_| args.repo_path.clone());
    let meta = WorkspaceMetadata::new(repo_path, args.branch_name.clone(), worktree_path.clone());
    if let Err(e) = meta.write(&rootdir) {
        let _ = branch.delete();
        let _ = std::fs::remove_dir_all(&rootdir);
        handle_error(ctx, e);
    }

    match ctx.output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "status": "ok",
                    "rootdir": rootdir.to_string_lossy()
                })
            );
        }
        OutputFormat::Text => {
            println!("{}", rootdir.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use uuid::Uuid;

    #[test]
    fn rootdir_has_expected_structure() {
        // Verify naming contract: sodagun_{repo}_{branch}_{uuid8}
        // `_` is the structural separator; components use `-` internally.
        let uuid8 = &Uuid::new_v4().to_string()[..8];
        let dir = PathBuf::from("/tmp");
        let rootdir = dir.join(format!("sodagun_myrepo_feature-x_{uuid8}"));
        let name = rootdir.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("sodagun_myrepo_feature-x_"), "name={name}");
        let suffix = name.trim_start_matches("sodagun_myrepo_feature-x_");
        assert_eq!(suffix.len(), 8, "suffix={suffix}");
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "suffix={suffix}"
        );
    }

    #[test]
    fn branch_sanitization() {
        // `/` and `_` in branch names both become `-`; `_` is reserved as the separator
        assert_eq!(
            "feature/my-thing".replace(['/', '_'], "-"),
            "feature-my-thing"
        );
        assert_eq!(
            "feat_underscore/sub".replace(['/', '_'], "-"),
            "feat-underscore-sub"
        );
    }
}
