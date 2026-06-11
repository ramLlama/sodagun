//! Synthesizes the mounts and env vars that give a sandbox guest git access.
//!
//! A linked worktree's `.git` file points into the host repository
//! (`gitdir: <repo>/.git/worktrees/<name>`), and that admin dir's
//! `commondir` file points at the shared `.git` — none of which exist in
//! the guest.  Rather than mirroring host paths, the shared `.git` is
//! mounted at `<working_dir>.git` (e.g. `/workspace.git`) and git is wired
//! up via environment variables injected at `sandbox start`:
//! `GIT_DIR=<working_dir>.git/worktrees/<name>`,
//! `GIT_COMMON_DIR=<working_dir>.git`, `GIT_WORK_TREE=<working_dir>`.
//! With those set, guest git never consults the on-disk pointer files.
//! See [`crate::config::GitAccess`] for the policy semantics.

use std::path::{Path, PathBuf};

use crate::config::GitAccess;
use crate::error::SodagunError;
use crate::git_meta::{common_git_dir, worktree_admin_dir};

/// One synthesized bind mount.
#[derive(Debug, PartialEq, Eq)]
pub struct GitMount {
    pub host: PathBuf,
    pub guest: String,
    pub readonly: bool,
}

/// Mounts (parents before nested children, the order the guest requires)
/// plus env vars implementing a [`GitAccess`] policy.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GitAccessSpec {
    pub mounts: Vec<GitMount>,
    pub env: Vec<(String, String)>,
}

fn invalid(message: String) -> SodagunError {
    SodagunError {
        code: "GIT_ACCESS_INVALID",
        message,
    }
}

/// Build the [`GitAccessSpec`] implementing ACCESS for WORKTREE, with the
/// worktree visible in the guest at WORKING_DIR.  Empty for `none`.
///
/// GITCONFIG, when given (the host `~/.gitconfig`, if it exists), is
/// mounted at `<working_dir>.gitconfig` and wired via `GIT_CONFIG_GLOBAL`
/// — guest homes differ from the host's, and this carries the user's
/// commit identity into the guest.  Read-only under `data`, read-write
/// under `full`.
pub fn git_access_spec(
    worktree: &Path,
    working_dir: &str,
    access: GitAccess,
    gitconfig: Option<&Path>,
) -> Result<GitAccessSpec, SodagunError> {
    if access == GitAccess::None {
        return Ok(GitAccessSpec::default());
    }
    let admin = worktree_admin_dir(worktree)?;
    let git_dir = common_git_dir(&admin)?;
    let admin_name = admin
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| invalid(format!("admin dir has no name: {}", admin.display())))?;

    let guest_git = format!("{}.git", working_dir.trim_end_matches('/'));
    let guest_admin = format!("{guest_git}/worktrees/{admin_name}");
    let guest_gitconfig = format!("{}.gitconfig", working_dir.trim_end_matches('/'));
    let mut env = vec![
        ("GIT_DIR".to_string(), guest_admin.clone()),
        ("GIT_COMMON_DIR".to_string(), guest_git.clone()),
        ("GIT_WORK_TREE".to_string(), working_dir.to_string()),
        // Injected git config (override wholesale via [sandbox.env] if needed):
        // - hooks off: repo hooks were installed for the HOST environment
        //   (their tooling is usually absent in the guest);
        // - auto-gc off: gc wants packed-refs.lock at the .git top level,
        //   which is read-only under the `data` policy.
        ("GIT_CONFIG_COUNT".to_string(), "2".to_string()),
        ("GIT_CONFIG_KEY_0".to_string(), "core.hooksPath".to_string()),
        ("GIT_CONFIG_VALUE_0".to_string(), "/dev/null".to_string()),
        ("GIT_CONFIG_KEY_1".to_string(), "gc.auto".to_string()),
        ("GIT_CONFIG_VALUE_1".to_string(), "0".to_string()),
    ];
    if gitconfig.is_some() {
        env.push(("GIT_CONFIG_GLOBAL".to_string(), guest_gitconfig.clone()));
    }

    let mut mounts = match access {
        GitAccess::None => unreachable!(),
        GitAccess::Full => vec![GitMount {
            host: git_dir,
            guest: guest_git,
            readonly: false,
        }],
        GitAccess::Data => {
            // `logs/` may not exist yet in a repo without reflog activity;
            // git creates it lazily, but a mount source must exist.
            let logs = git_dir.join("logs");
            std::fs::create_dir_all(&logs)
                .map_err(|e| invalid(format!("cannot create {}: {e}", logs.display())))?;
            vec![
                // Shared .git read-only: config, hooks/, packed-refs — every
                // host-code-execution surface stays unwritable.
                GitMount {
                    host: git_dir.clone(),
                    guest: guest_git.clone(),
                    readonly: true,
                },
                // Data-only surfaces a commit needs to write.
                GitMount {
                    host: git_dir.join("objects"),
                    guest: format!("{guest_git}/objects"),
                    readonly: false,
                },
                GitMount {
                    host: git_dir.join("refs"),
                    guest: format!("{guest_git}/refs"),
                    readonly: false,
                },
                GitMount {
                    host: logs,
                    guest: format!("{guest_git}/logs"),
                    readonly: false,
                },
                GitMount {
                    host: admin.clone(),
                    guest: guest_admin.clone(),
                    readonly: false,
                },
                // Pin the pointer files inside the rw admin dir, plus the
                // worktree's own .git file in the rw workspace mount: guest
                // git ignores them (env-wired), but HOST git reads them — a
                // guest rewriting either could aim host-side git at an
                // attacker-controlled .git whose config/hooks execute on
                // the host.
                GitMount {
                    host: admin.join("commondir"),
                    guest: format!("{guest_admin}/commondir"),
                    readonly: true,
                },
                GitMount {
                    host: admin.join("gitdir"),
                    guest: format!("{guest_admin}/gitdir"),
                    readonly: true,
                },
                GitMount {
                    host: worktree.join(".git"),
                    guest: format!("{}/.git", working_dir.trim_end_matches('/')),
                    readonly: true,
                },
            ]
        }
    };
    if let Some(gitconfig) = gitconfig {
        mounts.push(GitMount {
            host: gitconfig.to_path_buf(),
            guest: guest_gitconfig,
            readonly: access == GitAccess::Data,
        });
    }
    Ok(GitAccessSpec { mounts, env })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out a fake repo + linked worktree; returns (tmp, git_dir, worktree, admin).
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join("repo/.git");
        let admin = git_dir.join("worktrees/feat");
        let worktree = tmp.path().join("ws/feat");
        std::fs::create_dir_all(git_dir.join("objects")).unwrap();
        std::fs::create_dir_all(git_dir.join("refs")).unwrap();
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        std::fs::write(admin.join("gitdir"), "ignored\n").unwrap();
        let git_dir = git_dir.canonicalize().unwrap();
        let admin = admin.canonicalize().unwrap();
        (tmp, git_dir, worktree, admin)
    }

    #[test]
    fn none_yields_empty_spec() {
        let (_tmp, _git, worktree, _admin) = fixture();
        assert_eq!(
            git_access_spec(&worktree, "/workspace", GitAccess::None, None).unwrap(),
            GitAccessSpec::default()
        );
    }

    #[test]
    fn env_wires_git_at_guest_paths() {
        let (_tmp, _git, worktree, _admin) = fixture();
        let spec = git_access_spec(&worktree, "/workspace", GitAccess::Full, None).unwrap();
        let env: std::collections::HashMap<_, _> = spec.env.into_iter().collect();
        assert_eq!(
            env.get("GIT_DIR").map(String::as_str),
            Some("/workspace.git/worktrees/feat")
        );
        assert_eq!(
            env.get("GIT_COMMON_DIR").map(String::as_str),
            Some("/workspace.git")
        );
        assert_eq!(
            env.get("GIT_WORK_TREE").map(String::as_str),
            Some("/workspace")
        );
        // Hooks and auto-gc are disabled for guest git.
        assert_eq!(env.get("GIT_CONFIG_COUNT").map(String::as_str), Some("2"));
        assert_eq!(
            env.get("GIT_CONFIG_KEY_0").map(String::as_str),
            Some("core.hooksPath")
        );
        assert_eq!(
            env.get("GIT_CONFIG_KEY_1").map(String::as_str),
            Some("gc.auto")
        );
    }

    #[test]
    fn full_mounts_whole_git_dir_rw() {
        let (_tmp, git_dir, worktree, _admin) = fixture();
        let spec = git_access_spec(&worktree, "/workspace", GitAccess::Full, None).unwrap();
        assert_eq!(
            spec.mounts,
            vec![GitMount {
                host: git_dir,
                guest: "/workspace.git".to_string(),
                readonly: false
            }]
        );
    }

    #[test]
    fn data_layers_ro_git_with_rw_data_and_pinned_pointers() {
        let (_tmp, git_dir, worktree, admin) = fixture();
        let spec = git_access_spec(&worktree, "/workspace", GitAccess::Data, None).unwrap();
        let g = "/workspace.git";
        let expected = vec![
            GitMount {
                host: git_dir.clone(),
                guest: g.to_string(),
                readonly: true,
            },
            GitMount {
                host: git_dir.join("objects"),
                guest: format!("{g}/objects"),
                readonly: false,
            },
            GitMount {
                host: git_dir.join("refs"),
                guest: format!("{g}/refs"),
                readonly: false,
            },
            GitMount {
                host: git_dir.join("logs"),
                guest: format!("{g}/logs"),
                readonly: false,
            },
            GitMount {
                host: admin.clone(),
                guest: format!("{g}/worktrees/feat"),
                readonly: false,
            },
            GitMount {
                host: admin.join("commondir"),
                guest: format!("{g}/worktrees/feat/commondir"),
                readonly: true,
            },
            GitMount {
                host: admin.join("gitdir"),
                guest: format!("{g}/worktrees/feat/gitdir"),
                readonly: true,
            },
            GitMount {
                host: worktree.join(".git"),
                guest: "/workspace/.git".to_string(),
                readonly: true,
            },
        ];
        assert_eq!(spec.mounts, expected);
        // logs/ was created on demand so the mount source exists.
        assert!(git_dir.join("logs").is_dir());
    }

    #[test]
    fn missing_dotgit_pointer_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("plain");
        std::fs::create_dir_all(&worktree).unwrap();
        assert!(git_access_spec(&worktree, "/workspace", GitAccess::Data, None).is_err());
    }

    #[test]
    fn gitconfig_mounted_ro_for_data_rw_for_full() {
        let (tmp, _git, worktree, _admin) = fixture();
        let gitconfig = tmp.path().join("home/.gitconfig");
        std::fs::create_dir_all(gitconfig.parent().unwrap()).unwrap();
        std::fs::write(&gitconfig, "[user]\n\tname = ram\n").unwrap();

        for (access, readonly) in [(GitAccess::Data, true), (GitAccess::Full, false)] {
            let spec = git_access_spec(&worktree, "/workspace", access, Some(&gitconfig)).unwrap();
            let mount = spec
                .mounts
                .iter()
                .find(|m| m.guest == "/workspace.gitconfig")
                .expect("gitconfig mount present");
            assert_eq!(mount.host, gitconfig);
            assert_eq!(mount.readonly, readonly);
            assert!(spec.env.contains(&(
                "GIT_CONFIG_GLOBAL".to_string(),
                "/workspace.gitconfig".to_string()
            )));
        }
    }

    #[test]
    fn no_gitconfig_no_global_env() {
        let (_tmp, _git, worktree, _admin) = fixture();
        let spec = git_access_spec(&worktree, "/workspace", GitAccess::Data, None).unwrap();
        assert!(spec.env.iter().all(|(k, _)| k != "GIT_CONFIG_GLOBAL"));
        assert!(
            spec.mounts
                .iter()
                .all(|m| m.guest != "/workspace.gitconfig")
        );
    }
}
