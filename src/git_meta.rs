//! Helpers for resolving and normalizing linked-worktree git metadata.
//!
//! A linked worktree's `.git` file points at its admin dir
//! (`<repo>/.git/worktrees/<name>`), whose `commondir` file points at the
//! shared `.git`.  Used by `git add-worktree` (normalization) and the
//! sandbox `git_access` mount synthesis.

use std::path::{Path, PathBuf};

use crate::error::SodagunError;

fn invalid(message: String) -> SodagunError {
    SodagunError {
        code: "GIT_ERROR",
        message,
    }
}

/// Resolve the worktree's admin dir (`<repo>/.git/worktrees/<name>`) from
/// the `gitdir:` pointer in `<worktree>/.git`.
pub fn worktree_admin_dir(worktree: &Path) -> Result<PathBuf, SodagunError> {
    let dotgit = worktree.join(".git");
    let content = std::fs::read_to_string(&dotgit).map_err(|e| {
        invalid(format!(
            "cannot read {} (is this a linked worktree?): {e}",
            dotgit.display()
        ))
    })?;
    let pointer = content
        .strip_prefix("gitdir:")
        .ok_or_else(|| invalid(format!("{} has no gitdir pointer", dotgit.display())))?
        .trim();
    let admin = if Path::new(pointer).is_absolute() {
        PathBuf::from(pointer)
    } else {
        worktree.join(pointer)
    };
    admin
        .canonicalize()
        .map_err(|e| invalid(format!("worktree admin dir {}: {e}", admin.display())))
}

/// Resolve the shared `.git` dir from the admin dir's `commondir` file.
pub fn common_git_dir(admin: &Path) -> Result<PathBuf, SodagunError> {
    let commondir_file = admin.join("commondir");
    let content = std::fs::read_to_string(&commondir_file)
        .map_err(|e| invalid(format!("cannot read {}: {e}", commondir_file.display())))?;
    let pointer = content.trim();
    let common = if Path::new(pointer).is_absolute() {
        PathBuf::from(pointer)
    } else {
        admin.join(pointer)
    };
    common
        .canonicalize()
        .map_err(|e| invalid(format!("git common dir {}: {e}", common.display())))
}

/// Rewrite the worktree's `commondir` to the conventional relative `../..`.
///
/// libgit2 writes an absolute host path there; the git CLI writes `../..`.
/// The relative form is semantically identical (the admin dir always lives
/// at `<git-common-dir>/worktrees/<name>`) and keeps the worktree usable
/// when the repository is reached at a different path — e.g. mounted into
/// a sandbox guest, where some git codepaths read `commondir` even when
/// `GIT_COMMON_DIR` is set.
pub fn normalize_commondir(worktree: &Path) -> Result<(), SodagunError> {
    let admin = worktree_admin_dir(worktree)?;
    let commondir_file = admin.join("commondir");
    std::fs::write(&commondir_file, "../..\n")
        .map_err(|e| invalid(format!("cannot write {}: {e}", commondir_file.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join("repo/.git");
        let admin = git_dir.join("worktrees/feat");
        let worktree = tmp.path().join("ws/feat");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();
        // Absolute commondir, as libgit2 writes it.
        std::fs::write(admin.join("commondir"), format!("{}\n", git_dir.display())).unwrap();
        (tmp, worktree, admin)
    }

    #[test]
    fn normalize_commondir_writes_relative_pointer() {
        let (_tmp, worktree, admin) = fixture();
        normalize_commondir(&worktree).unwrap();
        assert_eq!(
            std::fs::read_to_string(admin.join("commondir")).unwrap(),
            "../..\n"
        );
        // Still resolves to the same shared .git.
        let common = common_git_dir(&admin.canonicalize().unwrap()).unwrap();
        assert!(common.ends_with("repo/.git"));
    }

    #[test]
    fn admin_dir_resolves_from_dotgit_pointer() {
        let (_tmp, worktree, admin) = fixture();
        assert_eq!(
            worktree_admin_dir(&worktree).unwrap(),
            admin.canonicalize().unwrap()
        );
    }
}
