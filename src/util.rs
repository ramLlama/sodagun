//! Small cross-cutting helpers shared across commands.
//!
//! Houses the microsandbox SDK ↔ sodagun translation layer (error/status mapping
//! plus the shared tokio runtime), keeping those mappings consistent across commands.

use std::sync::OnceLock;

use colored::Colorize;
use microsandbox::MicrosandboxError;
use microsandbox::sandbox::SandboxStatus;
use tokio::runtime::Runtime;

use crate::error::SodagunError;

// ── msb version guard ─────────────────────────────────────────────────────

/// Minimum `msb` binary version compatible with the `microsandbox` SDK version
/// in Cargo.toml. Update the minor when upgrading the SDK past a breaking
/// protocol change (the binary must be at least this version).
const REQUIRED_MSB_VERSION: (u32, u32, u32) = (0, 5, 0);

/// Verify that the resolved `msb` binary is at least [`REQUIRED_MSB_VERSION`].
///
/// The SDK resolution order prefers `~/.microsandbox/bin/msb` over the system
/// PATH, so a stale system-managed binary can silently shadow a current one and
/// produce cryptic protocol errors. Checking early surfaces a clear
/// "run: msb self update" message before any SDK call is made.
pub fn check_msb_version() -> Result<(), SodagunError> {
    let msb_path = microsandbox::config::resolve_msb_path().map_err(|e| SodagunError {
        code: "SANDBOX_ERROR",
        message: format!("could not resolve msb binary: {e}"),
    })?;

    let output = std::process::Command::new(&msb_path)
        .arg("--version")
        .output()
        .map_err(|e| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("failed to run `msb --version`: {e}"),
        })?;

    // Output is "msb X.Y.Z\n"; take the second whitespace token.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version_str = stdout
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("unexpected `msb --version` output: {stdout:?}"),
        })?;

    let actual = parse_msb_version(version_str).ok_or_else(|| SodagunError {
        code: "SANDBOX_ERROR",
        message: format!("could not parse `msb --version` output: {stdout:?}"),
    })?;

    if actual < REQUIRED_MSB_VERSION {
        return Err(SodagunError {
            code: "SANDBOX_ERROR",
            message: format!(
                "msb at {} is version {version_str}, but sodagun requires >= {}.{}.{} — \
                 run: msb self update",
                msb_path.display(),
                REQUIRED_MSB_VERSION.0,
                REQUIRED_MSB_VERSION.1,
                REQUIRED_MSB_VERSION.2,
            ),
        });
    }

    Ok(())
}

/// Parse a dotted version string (e.g. `"0.5.4"`) into a comparable triple.
fn parse_msb_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

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

    #[test]
    fn parse_msb_version_full() {
        assert_eq!(parse_msb_version("0.5.4"), Some((0, 5, 4)));
        assert_eq!(parse_msb_version("1.0.0"), Some((1, 0, 0)));
        assert_eq!(parse_msb_version("0.4.6"), Some((0, 4, 6)));
    }

    #[test]
    fn parse_msb_version_two_components_defaults_patch_to_zero() {
        assert_eq!(parse_msb_version("0.5"), Some((0, 5, 0)));
    }

    #[test]
    fn parse_msb_version_invalid_returns_none() {
        assert_eq!(parse_msb_version(""), None);
        assert_eq!(parse_msb_version("abc"), None);
        assert_eq!(parse_msb_version("v0.5.4"), None); // leading 'v' is not a digit
    }

    #[test]
    fn msb_version_comparison() {
        // Ensure tuple ordering matches semver expectations used in the guard.
        assert!((0, 5, 4) >= REQUIRED_MSB_VERSION); // current
        assert!((0, 4, 6) < REQUIRED_MSB_VERSION); // old system binary
        assert!((0, 6, 0) >= REQUIRED_MSB_VERSION); // future minor
        assert!((1, 0, 0) >= REQUIRED_MSB_VERSION); // future major
    }
}
