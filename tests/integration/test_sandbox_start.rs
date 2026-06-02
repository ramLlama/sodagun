use std::fs;
use std::path::Path;

use predicates::prelude::*;
use tempfile::TempDir;

use super::utils::{sodagun, sodagun_isolated};

/// Create a minimal workspace (rootdir + sodagun.json + worktree subdir) without
/// needing a real git repo. Sufficient for config-error path tests.
fn make_workspace(rootdir: &Path, branch: &str) {
    make_workspace_with_repo(rootdir, Path::new("/test/repo"), branch);
}

/// Like `make_workspace` but with an explicit repo_path (for fallback config tests).
fn make_workspace_with_repo(rootdir: &Path, repo_path: &Path, branch: &str) {
    fs::create_dir_all(rootdir).unwrap();
    let worktree = rootdir.join(branch);
    fs::create_dir(&worktree).unwrap();
    let meta = serde_json::json!({
        "version": 1,
        "repo_path": repo_path.to_str().unwrap(),
        "branch": branch,
        "created_at": "2026-01-01T00:00:00Z",
        "worktree_path": worktree.to_str().unwrap(),
        "sandbox_name": null
    });
    fs::write(rootdir.join("sodagun.json"), meta.to_string()).unwrap();
}

// --- WORKSPACE_NOT_FOUND ---

#[test]
fn workspace_not_found_text() {
    sodagun()
        .args(["sandbox", "start", "/nonexistent/rootdir"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("WORKSPACE_NOT_FOUND"));
}

#[test]
fn workspace_not_found_json() {
    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "start",
            "/nonexistent/rootdir",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("WORKSPACE_NOT_FOUND"));
}

// --- CONFIG_NOT_FOUND (explicit --config pointing to a missing file) ---

#[test]
fn config_not_found_explicit_text() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");

    sodagun()
        .args([
            "sandbox",
            "start",
            rootdir.to_str().unwrap(),
            "--config",
            "/nonexistent/sodagun.toml",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_NOT_FOUND"));
}

#[test]
fn config_not_found_explicit_json() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "start",
            rootdir.to_str().unwrap(),
            "--config",
            "/nonexistent/sodagun.toml",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_NOT_FOUND"));
}

// --- CONFIG_INVALID: malformed TOML ---

#[test]
fn config_invalid_bad_toml_text() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "not valid toml @@@@",
    )
    .unwrap();

    sodagun()
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

#[test]
fn config_invalid_bad_toml_json() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "not valid toml @@@@",
    )
    .unwrap();

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "start",
            rootdir.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_INVALID"));
}

// --- CONFIG_INVALID: base_image/base_snapshot conflict ---

#[test]
fn config_invalid_image_snapshot_conflict_text() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "[image]\nbase_image = \"debian\"\nbase_snapshot = \"my-snap\"\n",
    )
    .unwrap();

    sodagun()
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

#[test]
fn config_invalid_image_snapshot_conflict_json() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "[image]\nbase_image = \"debian\"\nbase_snapshot = \"my-snap\"\n",
    )
    .unwrap();

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "start",
            rootdir.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_INVALID"));
}

// --- Config resolution ---

/// When the worktree has a sodagun.toml, it takes precedence over the repo config.
/// Verified by giving the worktree bad TOML (→ CONFIG_INVALID) while the repo has valid TOML.
#[test]
fn config_resolution_worktree_over_repo() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    make_workspace_with_repo(&rootdir, &repo, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "not valid toml @@@@",
    )
    .unwrap();
    fs::write(
        repo.join("sodagun.toml"),
        "[image]\nbase_image = \"debian\"\n",
    )
    .unwrap();

    sodagun()
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

/// When the worktree has no sodagun.toml, the repo config is used as a fallback.
/// Verified by giving the repo bad TOML (→ CONFIG_INVALID) while the worktree has none.
#[test]
fn config_resolution_repo_fallback() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    make_workspace_with_repo(&rootdir, &repo, "feature");
    fs::write(repo.join("sodagun.toml"), "not valid toml @@@@").unwrap();

    sodagun()
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

// --- CONFIG_INVALID: value_from_cmd exits non-zero ---

/// A secret with `value_from_cmd = "exit 1"` should produce CONFIG_INVALID at launch,
/// not at parse time.
#[test]
fn config_invalid_value_from_cmd_nonzero_exit() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        r#"
[image]
base_image = "alpine"

[sandbox.secrets.MY_SECRET]
value_from_cmd = "exit 1"
allowed_hosts = []
"#,
    )
    .unwrap();

    sodagun()
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

// --- CONFIG_INVALID: unknown network policy name, no policies file ---

#[test]
fn config_invalid_unknown_policy_no_policies_file() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    // Set XDG_CONFIG_HOME to an empty dir so no network-policies.toml exists.
    let xdg_home = tmp.path().join("xdg");
    fs::create_dir_all(&xdg_home).unwrap();
    make_workspace(&rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "[image]\nbase_image = \"alpine\"\n[sandbox.network]\npolicy = \"my-nonexistent-policy\"\n",
    )
    .unwrap();

    sodagun()
        .env("XDG_CONFIG_HOME", xdg_home.to_str().unwrap())
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

// --- CONFIG_INVALID: env var value contains control characters ---

/// A `value_from_cmd` that emits multi-line output (e.g. `jq --raw-output` on a JSON array)
/// must be caught before VM launch and reported as CONFIG_INVALID.
/// Previously this caused SIGABRT in the VM because the env var passed to the SDK contained
/// embedded newlines.
#[test]
fn config_invalid_value_from_cmd_multiline_output() {
    let tmp = TempDir::new().unwrap();
    let xdg_tmp = TempDir::new().unwrap();
    let rootdir = tmp.path();
    make_workspace(rootdir, "feature");
    // `printf` emits two lines; simulates `jq --raw-output` on a JSON array.
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "[image]\nbase_image = \"alpine:latest\"\n[sandbox.env.MY_VAR]\nvalue_from_cmd = \"printf 'line1\\nline2'\"\n",
    )
    .unwrap();

    sodagun_isolated(&xdg_tmp)
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"))
        .stderr(predicate::str::contains("control character"));
}

/// A `value_from_cmd` that emits a trailing newline (the normal case for shell commands)
/// must be accepted — `trim_end()` strips it and the value is valid.
#[test]
fn config_invalid_value_from_cmd_trailing_newline_accepted() {
    let tmp = TempDir::new().unwrap();
    let xdg_tmp = TempDir::new().unwrap();
    let rootdir = tmp.path();
    make_workspace(rootdir, "feature");
    // `echo` adds a trailing newline; the value should trim to "hello" with no error.
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "[image]\nbase_image = \"alpine:latest\"\n[sandbox.env.MY_VAR]\nvalue_from_cmd = \"echo hello\"\n",
    )
    .unwrap();

    // This test only checks that the error is NOT about control characters; it may fail
    // for other reasons (e.g. no hardware virtualization) but must not fail with CONFIG_INVALID.
    let output = sodagun_isolated(&xdg_tmp)
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("CONFIG_INVALID"),
        "trailing newline should be trimmed silently, not rejected; stderr: {stderr}"
    );
    // Clean up if sandbox actually started.
    if output.status.success() {
        sodagun_isolated(&xdg_tmp)
            .args(["sandbox", "remove", rootdir.to_str().unwrap()])
            .assert()
            .success();
    }
}

// --- CONFIG_INVALID: reserved policy name redefined in network-policies.toml ---

/// `network-policies.toml` must not redefine reserved built-in names.
#[test]
fn config_invalid_reserved_policy_name_redefined_in_file() {
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    let xdg_home = tmp.path().join("xdg");
    let policies_dir = xdg_home.join("sodagun");
    fs::create_dir_all(&policies_dir).unwrap();
    // Attempt to redefine the built-in "none" policy.
    fs::write(
        policies_dir.join("network-policies.toml"),
        "[none]\ndefault_egress = \"allow\"\n",
    )
    .unwrap();
    make_workspace(&rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "[image]\nbase_image = \"alpine\"\n[sandbox.network]\npolicy = \"none\"\n",
    )
    .unwrap();

    sodagun()
        .env("XDG_CONFIG_HOME", xdg_home.to_str().unwrap())
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"))
        .stderr(predicate::str::contains("reserved"));
}

// --- Full start tests (require KVM / Apple Silicon hardware virtualization) ---

#[test]
fn start_creates_sandbox() {
    // Use tmp.path() directly so the sandbox name is the unique tmpXXX dirname.
    let tmp = TempDir::new().unwrap();
    let xdg_tmp = TempDir::new().unwrap();
    let rootdir = tmp.path();
    make_workspace(rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "[image]\nbase_image = \"debian\"\n",
    )
    .unwrap();

    let output = sodagun_isolated(&xdg_tmp)
        .args([
            "--output",
            "json",
            "sandbox",
            "start",
            rootdir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let data: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(data["status"], "ok");
    assert!(data["sandbox_name"].as_str().is_some());

    // sodagun.json is updated with the sandbox name
    let raw = fs::read_to_string(rootdir.join("sodagun.json")).unwrap();
    let meta: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(meta["sandbox_name"], data["sandbox_name"]);

    sodagun_isolated(&xdg_tmp)
        .args(["sandbox", "remove", rootdir.to_str().unwrap()])
        .assert()
        .success();
}
