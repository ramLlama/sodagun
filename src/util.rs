//! Small cross-cutting helpers shared across commands.
//!
//! This also currently houses the microsandbox SDK ↔ sodagun translation layer
//! (error/status mapping plus the shared tokio runtime), so those mappings stay
//! consistent across the `sandbox` and `snapshot` commands instead of being
//! duplicated per file. Once the microsandbox-specific helpers grow, split them
//! out into a dedicated module.

use std::sync::OnceLock;

use colored::Colorize;
use microsandbox::MicrosandboxError;
use microsandbox::sandbox::SandboxStatus;
use tokio::runtime::Runtime;

use crate::error::SodagunError;

// ── Name sanitization ──────────────────────────────────────────────────────

/// Characters collapsed to `-` by [`dashify`]. `_` and `/` are structural
/// separators in sodagun's generated names (`sodagun_<repo>_<branch>_<uuid>`,
/// `<base>_<hash>`), and `:`/`@`/space appear in image refs; replacing them all
/// keeps the surrounding name unambiguous and shell-friendly.
const DASH_CHARS: &[char] = &['/', '_', ':', '@', ' '];

/// Collapse the standard set of separator/unsafe characters in `s` to `-`.
///
/// Used wherever a user-supplied component (repo name, branch, image ref) is
/// embedded into a generated, delimiter-bearing name.
pub fn dashify(s: &str) -> String {
    s.replace(DASH_CHARS, "-")
}

// ── microsandbox SDK boundary ──────────────────────────────────────────────

/// Returns the process-wide tokio runtime, building it on first use.
///
/// Only `sandbox`/`snapshot` commands touch the async SDK, so the runtime is
/// created lazily — `git` subcommands never pay for it.
///
/// Failing to construct it is unrecoverable (the OS can't hand us threads). We
/// exit directly here rather than routing through `handle_error`, which means
/// this one path does *not* emit a JSON error envelope under `--output json`.
/// That's a deliberate tradeoff to keep the accessor a zero-argument singleton:
/// runtime-build failure is catastrophic and effectively unreachable, and this
/// mirrors `find_project_dir`'s pre-command error handling (also plain stderr).
pub fn get_runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Runtime::new().unwrap_or_else(|e| {
            eprintln!(
                "{} failed to initialize async runtime: {e}",
                "error:".red().bold()
            );
            std::process::exit(1);
        })
    })
}

/// Maps a microsandbox error from a sandbox operation: an unknown sandbox name
/// becomes `SANDBOX_NOT_FOUND`, everything else `SANDBOX_ERROR`.
pub fn map_sandbox_err(e: MicrosandboxError, sandbox_name: &str) -> SodagunError {
    if matches!(e, MicrosandboxError::SandboxNotFound(_)) {
        SodagunError {
            code: "SANDBOX_NOT_FOUND",
            message: format!("sandbox '{sandbox_name}' not found"),
        }
    } else {
        SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("{e}"),
        }
    }
}

/// Maps a microsandbox error from a snapshot operation: an unknown snapshot name
/// becomes `SNAPSHOT_NOT_FOUND`, everything else `SNAPSHOT_ERROR`.
pub fn map_snapshot_err(e: MicrosandboxError, snapshot_name: &str) -> SodagunError {
    if matches!(e, MicrosandboxError::SnapshotNotFound(_)) {
        SodagunError {
            code: "SNAPSHOT_NOT_FOUND",
            message: format!("snapshot '{snapshot_name}' not found"),
        }
    } else {
        SodagunError {
            code: "SNAPSHOT_ERROR",
            message: format!("{e}"),
        }
    }
}

/// Lowercase, human-readable label for a sandbox status.
pub fn status_label(s: SandboxStatus) -> &'static str {
    match s {
        SandboxStatus::Running => "running",
        SandboxStatus::Draining => "draining",
        SandboxStatus::Paused => "paused",
        SandboxStatus::Stopped => "stopped",
        SandboxStatus::Crashed => "crashed",
    }
}

/// Whether a sandbox status is terminal (no longer transitioning).
pub fn is_terminal_status(s: SandboxStatus) -> bool {
    matches!(s, SandboxStatus::Stopped | SandboxStatus::Crashed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashify_collapses_separators() {
        assert_eq!(dashify("feature/my-thing"), "feature-my-thing");
        assert_eq!(dashify("feat_underscore/sub"), "feat-underscore-sub");
        assert_eq!(dashify("alpine:latest"), "alpine-latest");
        assert_eq!(dashify("ghcr.io/foo/bar:v1"), "ghcr.io-foo-bar-v1");
    }

    #[test]
    fn dashify_preserves_dots_and_hyphens() {
        // `.` and existing `-` are not separators, so they survive.
        assert_eq!(dashify("v1.2.3-rc1"), "v1.2.3-rc1");
    }
}
