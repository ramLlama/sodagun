use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn sodagun() -> Command {
    Command::cargo_bin("sodagun").unwrap()
}

/// Write a `.sodagun.toml` to a temp directory and return the dir.
fn config_dir(content: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join(".sodagun.toml"), content).unwrap();
    tmp
}

/// Create a minimal workspace (rootdir + sodagun.json + worktree subdir).
fn make_workspace(rootdir: &Path, worktree_name: &str) {
    fs::create_dir_all(rootdir).unwrap();
    let worktree = rootdir.join(worktree_name);
    fs::create_dir_all(&worktree).unwrap();
    let meta = serde_json::json!({
        "version": 1,
        "repo_path": "/test/repo",
        "branch": worktree_name,
        "created_at": "2026-01-01T00:00:00Z",
        "worktree_path": worktree.to_str().unwrap(),
        "sandbox_name": null,
    });
    fs::write(rootdir.join("sodagun.json"), meta.to_string()).unwrap();
}

// ── CONFIG_NOT_FOUND ──────────────────────────────────────────────────────────

#[test]
fn snapshot_create_config_not_found_text() {
    sodagun()
        .args([
            "--project-dir",
            "/nonexistent/rootdir",
            "snapshot",
            "create",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_NOT_FOUND"));
}

#[test]
fn snapshot_create_config_not_found_json() {
    sodagun()
        .args([
            "--project-dir",
            "/nonexistent/rootdir",
            "--output",
            "json",
            "snapshot",
            "create",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_NOT_FOUND"));
}

// ── CONFIG_INVALID: malformed TOML ───────────────────────────────────────────

#[test]
fn snapshot_create_invalid_toml_text() {
    let tmp = config_dir("not valid toml @@@@");
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "snapshot",
            "create",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

#[test]
fn snapshot_create_invalid_toml_json() {
    let tmp = config_dir("not valid toml @@@@");
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "--output",
            "json",
            "snapshot",
            "create",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_INVALID"));
}

// ── CONFIG_INVALID: missing [image] section ───────────────────────────────────

#[test]
fn snapshot_create_missing_image_section_text() {
    let tmp = config_dir("[sandbox]\nworking_dir = \"/workspace\"\n");
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "snapshot",
            "create",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

#[test]
fn snapshot_create_missing_image_section_json() {
    let tmp = config_dir("[sandbox]\nworking_dir = \"/workspace\"\n");
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "--output",
            "json",
            "snapshot",
            "create",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_INVALID"));
}

// ── CONFIG_INVALID: base_image + base_snapshot conflict ───────────────────────

#[test]
fn snapshot_create_base_conflict_text() {
    let tmp = config_dir("[image]\nbase_image = \"alpine\"\nbase_snapshot = \"snap\"\n");
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "snapshot",
            "create",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

#[test]
fn snapshot_create_base_conflict_json() {
    let tmp = config_dir("[image]\nbase_image = \"alpine\"\nbase_snapshot = \"snap\"\n");
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "--output",
            "json",
            "snapshot",
            "create",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_INVALID"));
}

// ── CONFIG_INVALID: no setup_script in [image] ────────────────────────────────

#[test]
fn snapshot_create_no_setup_script_text() {
    // [image] with only base_image — valid config but snapshot create requires a script.
    let tmp = config_dir("[image]\nbase_image = \"alpine:latest\"\n");
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "snapshot",
            "create",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

#[test]
fn snapshot_create_no_setup_script_json() {
    let tmp = config_dir("[image]\nbase_image = \"alpine:latest\"\n");
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "--output",
            "json",
            "snapshot",
            "create",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_INVALID"));
}

// ── SNAPSHOT_NOT_FOUND: remove non-existent snapshot ──────────────────────────

/// Config with a setup_script whose derived snapshot name won't exist.
fn nonexistent_snapshot_dir() -> TempDir {
    config_dir(
        "[image]\nbase_image = \"alpine:latest\"\nsetup_script = \"#!/bin/sh\\necho nonexistent\\n\"\n",
    )
}

#[test]
fn snapshot_remove_not_found_text() {
    let tmp = nonexistent_snapshot_dir();
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "snapshot",
            "remove",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("SNAPSHOT_NOT_FOUND"));
}

#[test]
fn snapshot_remove_not_found_json() {
    let tmp = nonexistent_snapshot_dir();
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "--output",
            "json",
            "snapshot",
            "remove",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("SNAPSHOT_NOT_FOUND"));
}

// ── snapshot remove --force succeeds even when snapshot doesn't exist ─────────

#[test]
fn snapshot_remove_force_nonexistent_succeeds() {
    let tmp = nonexistent_snapshot_dir();
    sodagun()
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "snapshot",
            "remove",
            "--force",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed."));
}

// ── Happy-path tests (require KVM / Apple Silicon hvf) ────────────────────────

#[test]
fn snapshot_create_and_idempotent() {
    let setup_script = "#!/bin/sh\napk add --no-cache git\n";
    let toml = format!(
        "[image]\nbase_image = \"alpine:latest\"\nsetup_script = {:?}\n",
        setup_script
    );
    let tmp = config_dir(&toml);
    let rootdir = tmp.path().to_str().unwrap();

    // Clear any stale snapshot from a previous run.
    sodagun()
        .args(["--project-dir", rootdir, "snapshot", "remove", "--force"])
        .assert()
        .success();

    // First create should succeed.
    let output = sodagun()
        .args([
            "--project-dir",
            rootdir,
            "--output",
            "json",
            "snapshot",
            "create",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let data: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(data["status"], "ok");
    let snap_name = data["snapshot_name"].as_str().unwrap().to_string();
    assert!(!snap_name.is_empty());
    assert_eq!(data["already_existed"], false);

    // Second create (no --force) should report already_existed = true.
    let output2 = sodagun()
        .args([
            "--project-dir",
            rootdir,
            "--output",
            "json",
            "snapshot",
            "create",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let data2: serde_json::Value = serde_json::from_slice(&output2).unwrap();
    assert_eq!(data2["already_existed"], true);

    // Clean up.
    sodagun()
        .args(["--project-dir", rootdir, "snapshot", "remove"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed."));
}

#[test]
fn snapshot_create_force_recreates() {
    let setup_script = "#!/bin/sh\napk add --no-cache curl\n";
    let toml = format!(
        "[image]\nbase_image = \"alpine:latest\"\nsetup_script = {:?}\n",
        setup_script
    );
    let tmp = config_dir(&toml);
    let rootdir = tmp.path().to_str().unwrap();

    // Create once.
    sodagun()
        .args(["--project-dir", rootdir, "snapshot", "create"])
        .assert()
        .success();

    // Force recreate should succeed.
    sodagun()
        .args(["--project-dir", rootdir, "snapshot", "create", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created snapshot:"));

    sodagun()
        .args(["--project-dir", rootdir, "snapshot", "remove"])
        .assert()
        .success();
}

// ── E2E: verify setup script side effects persist in the snapshot ─────────────

/// Creates a snapshot that installs git, boots a sandbox from it, and asserts
/// that `git version` succeeds — proving the setup script ran and its effects
/// are baked into the snapshot.
#[test]
fn snapshot_setup_script_side_effects_persist() {
    // 1. Build a snapshot that installs git on top of alpine:latest.
    let setup_script = "#!/bin/sh\nset -e\napk add --no-cache git\n";
    let snap_cfg = config_dir(&format!(
        "[image]\nbase_image = \"alpine:latest\"\nsetup_script = {setup_script:?}\n"
    ));

    let snap_output = sodagun()
        .args([
            "--project-dir",
            snap_cfg.path().to_str().unwrap(),
            "--output",
            "json",
            "snapshot",
            "create",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let snap_data: serde_json::Value = serde_json::from_slice(&snap_output).unwrap();
    let snap_name = snap_data["snapshot_name"].as_str().unwrap().to_string();

    // 2. Create a workspace whose [image] boots from that snapshot (no setup script
    //    of its own — this exercises the base_snapshot path in sandbox start).
    let ws_tmp = TempDir::new().unwrap();
    let rootdir = ws_tmp.path();
    make_workspace(rootdir, "worktree");
    fs::write(
        rootdir.join("worktree").join(".sodagun.toml"),
        format!("[image]\nbase_snapshot = {snap_name:?}\n"),
    )
    .unwrap();

    // 3. Start the sandbox.
    sodagun()
        .args(["sandbox", "start", rootdir.to_str().unwrap()])
        .assert()
        .success();

    // 4. Verify git is present — it was installed by the setup script, not in base alpine.
    let exec_output = sodagun()
        .args([
            "--output",
            "json",
            "sandbox",
            "exec",
            rootdir.to_str().unwrap(),
            "git",
            "version",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let exec_data: serde_json::Value = serde_json::from_slice(&exec_output).unwrap();
    assert_eq!(exec_data["exit_code"], 0, "git version should exit 0");
    assert!(
        exec_data["stdout"]
            .as_str()
            .unwrap_or("")
            .contains("git version"),
        "expected 'git version' in stdout, got: {}",
        exec_data["stdout"]
    );

    // 5. Tear down sandbox and snapshot.
    sodagun()
        .args(["sandbox", "remove", rootdir.to_str().unwrap()])
        .assert()
        .success();
    sodagun()
        .args([
            "--project-dir",
            snap_cfg.path().to_str().unwrap(),
            "snapshot",
            "remove",
        ])
        .assert()
        .success();
}
