use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn sodagun() -> Command {
    Command::cargo_bin("sodagun").unwrap()
}

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

// --- Full start test (requires KVM / Apple Silicon hardware virtualization) ---

#[test]
fn start_creates_sandbox() {
    // Use tmp.path() directly so the sandbox name is the unique tmpXXX dirname.
    let tmp = TempDir::new().unwrap();
    let rootdir = tmp.path();
    make_workspace(rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "[image]\nbase_image = \"debian\"\n",
    )
    .unwrap();

    let output = sodagun()
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

    sodagun()
        .args(["sandbox", "remove", rootdir.to_str().unwrap()])
        .assert()
        .success();
}
