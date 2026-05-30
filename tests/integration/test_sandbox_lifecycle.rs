use assert_cmd::Command;
use predicates::prelude::*;

fn sodagun() -> Command {
    Command::cargo_bin("sodagun").unwrap()
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

// --- sandbox stop: SANDBOX_NOT_FOUND ---

#[test]
fn stop_not_found_text() {
    sodagun()
        .args(["sandbox", "stop", "sodagun-sb-nonexistent-00000000"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("SANDBOX_NOT_FOUND"));
}

#[test]
fn stop_not_found_json() {
    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "stop",
            "sodagun-sb-nonexistent-00000000",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("SANDBOX_NOT_FOUND"));
}

// --- sandbox remove: SANDBOX_NOT_FOUND ---

#[test]
fn remove_not_found_text() {
    sodagun()
        .args(["sandbox", "remove", "sodagun-sb-nonexistent-00000000"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("SANDBOX_NOT_FOUND"));
}

#[test]
fn remove_not_found_json() {
    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "remove",
            "sodagun-sb-nonexistent-00000000",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("SANDBOX_NOT_FOUND"));
}

// --- Happy-path tests (require KVM / Apple Silicon hvf) ---

#[test]
#[ignore = "requires KVM or Apple Silicon hvf, and a valid image"]
fn stop_running_sandbox() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();

    // Launch and capture the sandbox name
    let output = sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "launch",
            worktree.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let name = json["sandbox_name"].as_str().unwrap();

    sodagun()
        .args(["sandbox", "stop", name])
        .assert()
        .success()
        .stdout(predicate::str::contains("Stopped."));
}

#[test]
#[ignore = "requires KVM or Apple Silicon hvf, and a valid image"]
fn remove_running_sandbox_implicit_stop() {
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();

    let output = sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "launch",
            worktree.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let name = json["sandbox_name"].as_str().unwrap();

    sodagun()
        .args(["sandbox", "remove", name])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed."));
}
