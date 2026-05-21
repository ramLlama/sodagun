use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn sodagun() -> Command {
    Command::cargo_bin("sodagun").unwrap()
}

// --- CONFIG_NOT_FOUND ---

#[test]
fn config_not_found_text() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    // No .sodagun.toml written

    sodagun()
        .args(["sandbox", "launch", worktree.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_NOT_FOUND"));
}

#[test]
fn config_not_found_json() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "launch",
            worktree.to_str().unwrap(),
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
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    fs::write(worktree.join(".sodagun.toml"), "not valid toml @@@@").unwrap();

    sodagun()
        .args(["sandbox", "launch", worktree.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

#[test]
fn config_invalid_bad_toml_json() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    fs::write(worktree.join(".sodagun.toml"), "not valid toml @@@@").unwrap();

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "launch",
            worktree.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_INVALID"));
}

// --- CONFIG_INVALID: image/snapshot conflict ---

#[test]
fn config_invalid_image_snapshot_conflict_text() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    fs::write(
        worktree.join(".sodagun.toml"),
        "[sandbox]\nimage = \"debian\"\nsnapshot = \"my-snap\"\n",
    )
    .unwrap();

    sodagun()
        .args(["sandbox", "launch", worktree.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

#[test]
fn config_invalid_image_snapshot_conflict_json() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    fs::write(
        worktree.join(".sodagun.toml"),
        "[sandbox]\nimage = \"debian\"\nsnapshot = \"my-snap\"\n",
    )
    .unwrap();

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "launch",
            worktree.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_INVALID"));
}

// --- Full launch test (requires KVM / Apple Silicon hardware virtualization) ---

#[test]
#[ignore = "requires KVM or Apple Silicon hvf, and a valid image"]
fn launch_creates_sandbox() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    fs::write(
        worktree.join(".sodagun.toml"),
        "[sandbox]\nimage = \"debian\"\n",
    )
    .unwrap();

    sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "launch",
            worktree.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("sandbox_name"));
}
