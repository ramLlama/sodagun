use std::path::{Path, PathBuf};

use assert_cmd::Command;
use git2::{Repository, Signature};
use predicates::prelude::*;
use tempfile::TempDir;

fn sodagun() -> Command {
    Command::cargo_bin("sodagun").unwrap()
}

/// Create a minimal git repo with one commit and an `origin/main` remote ref.
/// Mirrors the Python `git_repo` pytest fixture exactly.
fn make_git_repo(tmp: &TempDir) -> Repository {
    let repo_path = tmp.path().join("repo");
    std::fs::create_dir(&repo_path).unwrap();
    let repo = Repository::init(&repo_path).unwrap();

    let (repo_path_str, readme_path) = (repo_path.to_str().unwrap(), repo_path.join("README"));
    std::fs::write(&readme_path, "hello").unwrap();

    let mut index = repo.index().unwrap();
    index.add_path(Path::new("README")).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let sig = Signature::now("Test", "test@example.com").unwrap();
    let oid = repo
        .commit(
            Some("refs/heads/main"),
            &sig,
            &sig,
            "initial commit",
            &tree,
            &[],
        )
        .unwrap();
    drop(tree);

    // Create origin/main so the default --base resolves
    repo.reference("refs/remotes/origin/main", oid, false, "")
        .unwrap();

    let _ = repo_path_str; // suppress unused warning
    repo
}

fn workdir(repo: &Repository) -> PathBuf {
    PathBuf::from(
        repo.workdir()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_end_matches('/'),
    )
}

// --- Success cases ---

#[test]
fn default_creates_worktree_under_tmp() {
    let tmp = TempDir::new().unwrap();
    let repo = make_git_repo(&tmp);
    let wd = workdir(&repo);

    let output = sodagun()
        .args(["git", "add-worktree", wd.to_str().unwrap(), "feature-a"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let wt_path = PathBuf::from(String::from_utf8(output).unwrap().trim());
    assert!(wt_path.exists(), "worktree path should exist on disk");
    assert!(
        wt_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("sodagun-wt-repo-"),
        "worktree name should start with sodagun-wt-repo-"
    );
    assert_eq!(
        wt_path.parent().unwrap(),
        std::env::temp_dir(),
        "worktree should be under system temp dir by default"
    );
}

#[test]
fn custom_dir_prefix() {
    let tmp = TempDir::new().unwrap();
    let repo = make_git_repo(&tmp);
    let wd = workdir(&repo);
    let prefix = tmp.path().join("worktrees");
    std::fs::create_dir(&prefix).unwrap();

    let output = sodagun()
        .args([
            "git",
            "add-worktree",
            wd.to_str().unwrap(),
            "feature-b",
            "--dir-prefix",
            prefix.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let wt_path = PathBuf::from(String::from_utf8(output).unwrap().trim());
    assert!(wt_path.exists());
    assert_eq!(wt_path.parent().unwrap(), prefix);
    assert!(
        wt_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("sodagun-wt-repo-")
    );
}

#[test]
fn json_success_output() {
    let tmp = TempDir::new().unwrap();
    let repo = make_git_repo(&tmp);
    let wd = workdir(&repo);

    let output = sodagun()
        .args([
            "--output",
            "json",
            "git",
            "add-worktree",
            wd.to_str().unwrap(),
            "feature-c",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let data: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(data["status"], "ok");
    let wt_path = PathBuf::from(data["worktree_path"].as_str().unwrap());
    assert!(wt_path.exists());
}

// --- Error cases ---

#[test]
fn repo_not_found_text() {
    sodagun()
        .args(["git", "add-worktree", "/nonexistent/path", "branch"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("REPO_NOT_FOUND"));
}

#[test]
fn repo_not_found_json() {
    let output = sodagun()
        .args([
            "--output",
            "json",
            "git",
            "add-worktree",
            "/nonexistent/path",
            "branch",
        ])
        .assert()
        .failure()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let data: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        data,
        serde_json::json!({"status": "error", "code": "REPO_NOT_FOUND"})
    );
}

#[test]
fn base_not_found() {
    let tmp = TempDir::new().unwrap();
    let repo = make_git_repo(&tmp);
    let wd = workdir(&repo);

    sodagun()
        .args([
            "git",
            "add-worktree",
            wd.to_str().unwrap(),
            "branch",
            "--base",
            "refs/heads/nonexistent",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("BASE_NOT_FOUND"));
}

#[test]
fn base_not_found_json() {
    let tmp = TempDir::new().unwrap();
    let repo = make_git_repo(&tmp);
    let wd = workdir(&repo);

    let output = sodagun()
        .args([
            "--output",
            "json",
            "git",
            "add-worktree",
            wd.to_str().unwrap(),
            "branch",
            "--base",
            "refs/heads/nonexistent",
        ])
        .assert()
        .failure()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let data: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        data,
        serde_json::json!({"status": "error", "code": "BASE_NOT_FOUND"})
    );
}

#[test]
fn branch_already_exists() {
    let tmp = TempDir::new().unwrap();
    let repo = make_git_repo(&tmp);
    let wd = workdir(&repo);

    // Create the branch so it already exists
    sodagun()
        .args([
            "git",
            "add-worktree",
            wd.to_str().unwrap(),
            "existing-branch",
        ])
        .assert()
        .success();

    // Now try to create it again (new worktree path, but same branch name)
    sodagun()
        .args([
            "git",
            "add-worktree",
            wd.to_str().unwrap(),
            "existing-branch",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("BRANCH_EXISTS"));
}

#[test]
fn branch_already_exists_json() {
    let tmp = TempDir::new().unwrap();
    let repo = make_git_repo(&tmp);
    let wd = workdir(&repo);

    sodagun()
        .args([
            "git",
            "add-worktree",
            wd.to_str().unwrap(),
            "existing-branch-2",
        ])
        .assert()
        .success();

    let output = sodagun()
        .args([
            "--output",
            "json",
            "git",
            "add-worktree",
            wd.to_str().unwrap(),
            "existing-branch-2",
        ])
        .assert()
        .failure()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let data: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        data,
        serde_json::json!({"status": "error", "code": "BRANCH_EXISTS"})
    );
}

#[test]
fn no_args_exits_with_help() {
    sodagun().assert().failure().code(2);
}
