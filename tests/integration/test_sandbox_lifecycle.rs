use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

fn sodagun() -> Command {
    Command::cargo_bin("sodagun").unwrap()
}

fn sodagun_isolated(xdg_tmp: &tempfile::TempDir) -> Command {
    let mut cmd = Command::cargo_bin("sodagun").unwrap();
    cmd.env("XDG_CONFIG_HOME", xdg_tmp.path());
    cmd
}

/// Creates a minimal workspace with sandbox_name set to null.
fn make_workspace(rootdir: &Path, branch: &str) {
    fs::create_dir_all(rootdir).unwrap();
    let worktree = rootdir.join(branch);
    fs::create_dir(&worktree).unwrap();
    let meta = serde_json::json!({
        "version": 1,
        "repo_path": "/test/repo",
        "branch": branch,
        "created_at": "2026-01-01T00:00:00Z",
        "worktree_path": worktree.to_str().unwrap(),
        "sandbox_name": null
    });
    fs::write(rootdir.join("sodagun.json"), meta.to_string()).unwrap();
}

/// Creates a workspace with sandbox_name already set (simulates a launched sandbox).
fn make_workspace_with_sandbox(rootdir: &Path, branch: &str, sandbox_name: &str) {
    fs::create_dir_all(rootdir).unwrap();
    let worktree = rootdir.join(branch);
    fs::create_dir(&worktree).unwrap();
    let meta = serde_json::json!({
        "version": 1,
        "repo_path": "/test/repo",
        "branch": branch,
        "created_at": "2026-01-01T00:00:00Z",
        "worktree_path": worktree.to_str().unwrap(),
        "sandbox_name": sandbox_name
    });
    fs::write(rootdir.join("sodagun.json"), meta.to_string()).unwrap();
}

// --- sandbox list ---

#[test]
fn list_returns_ok_json_shape() {
    sodagun()
        .args(["--output", "json", "sandbox", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\":\"ok\""))
        .stdout(predicate::str::contains("\"sandboxes\":["));
}

#[test]
fn list_text_prints_header() {
    sodagun()
        .args(["sandbox", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("NAME"))
        .stdout(predicate::str::contains("STATUS"));
}

// --- sandbox stop: WORKSPACE_NOT_FOUND ---

#[test]
fn stop_workspace_not_found_text() {
    sodagun()
        .args(["sandbox", "stop", "/nonexistent/rootdir"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("WORKSPACE_NOT_FOUND"));
}

#[test]
fn stop_workspace_not_found_json() {
    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "stop",
            "/nonexistent/rootdir",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("WORKSPACE_NOT_FOUND"));
}

// --- sandbox stop: SANDBOX_NOT_STARTED (sandbox_name is null) ---

#[test]
fn stop_not_started_text() {
    let tmp = tempfile::TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");

    sodagun()
        .args(["sandbox", "stop", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("SANDBOX_NOT_STARTED"));
}

#[test]
fn stop_not_started_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "stop",
            rootdir.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("SANDBOX_NOT_STARTED"));
}

// --- sandbox stop: SANDBOX_NOT_FOUND (sandbox_name set but sandbox doesn't exist) ---

#[test]
fn stop_sandbox_not_found_text() {
    let tmp = tempfile::TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace_with_sandbox(&rootdir, "feature", "sodagun-nonexistent-00000000");

    sodagun()
        .args(["sandbox", "stop", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("SANDBOX_NOT_FOUND"));
}

#[test]
fn stop_sandbox_not_found_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace_with_sandbox(&rootdir, "feature", "sodagun-nonexistent-00000000");

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "stop",
            rootdir.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("SANDBOX_NOT_FOUND"));
}

// --- sandbox remove: WORKSPACE_NOT_FOUND ---

#[test]
fn remove_workspace_not_found_text() {
    sodagun()
        .args(["sandbox", "remove", "/nonexistent/rootdir"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("WORKSPACE_NOT_FOUND"));
}

#[test]
fn remove_workspace_not_found_json() {
    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "remove",
            "/nonexistent/rootdir",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("WORKSPACE_NOT_FOUND"));
}

// --- sandbox remove: SANDBOX_NOT_STARTED ---

#[test]
fn remove_not_started_text() {
    let tmp = tempfile::TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");

    sodagun()
        .args(["sandbox", "remove", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("SANDBOX_NOT_STARTED"));
}

#[test]
fn remove_not_started_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace(&rootdir, "feature");

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "remove",
            rootdir.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("SANDBOX_NOT_STARTED"));
}

// --- sandbox remove: SANDBOX_NOT_FOUND ---

#[test]
fn remove_sandbox_not_found_text() {
    let tmp = tempfile::TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace_with_sandbox(&rootdir, "feature", "sodagun-nonexistent-00000000");

    sodagun()
        .args(["sandbox", "remove", rootdir.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("SANDBOX_NOT_FOUND"));
}

#[test]
fn remove_sandbox_not_found_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    let rootdir = tmp.path().join("workspace");
    make_workspace_with_sandbox(&rootdir, "feature", "sodagun-nonexistent-00000000");

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "remove",
            rootdir.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("SANDBOX_NOT_FOUND"));
}

// --- Happy-path tests (require KVM / Apple Silicon hvf) ---

#[test]
fn stop_running_sandbox() {
    // Use tmp.path() directly as the rootdir so the sandbox name is the unique tmpXXX
    // dirname rather than a hardcoded "workspace" that collides across concurrent runs.
    let tmp = tempfile::TempDir::new().unwrap();
    let xdg_tmp = tempfile::TempDir::new().unwrap();
    let rootdir = tmp.path();
    make_workspace(rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "[image]\nbase_image = \"debian\"\n",
    )
    .unwrap();

    sodagun_isolated(&xdg_tmp)
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .success();

    sodagun_isolated(&xdg_tmp)
        .args(["sandbox", "stop", rootdir.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Stopped."));

    // Clean up so the stopped (but not removed) sandbox doesn't linger across runs.
    sodagun_isolated(&xdg_tmp)
        .args(["sandbox", "remove", rootdir.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn remove_running_sandbox_implicit_stop() {
    let tmp = tempfile::TempDir::new().unwrap();
    let xdg_tmp = tempfile::TempDir::new().unwrap();
    let rootdir = tmp.path();
    make_workspace(rootdir, "feature");
    fs::write(
        rootdir.join("feature").join("sodagun.toml"),
        "[image]\nbase_image = \"debian\"\n",
    )
    .unwrap();

    sodagun_isolated(&xdg_tmp)
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .success();

    sodagun_isolated(&xdg_tmp)
        .args(["sandbox", "remove", rootdir.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed."));
}
