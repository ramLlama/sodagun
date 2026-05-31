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
fn default_creates_workspace_under_tmp() {
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

    let rootdir = PathBuf::from(String::from_utf8(output).unwrap().trim());

    // rootdir is under system temp dir and follows the naming convention
    assert_eq!(
        rootdir.parent().unwrap(),
        std::env::temp_dir(),
        "rootdir should be under system temp dir by default"
    );
    assert!(
        rootdir
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("sodagun_repo_feature-a_"),
        "rootdir name should follow sodagun_{{repo}}_{{branch}}_{{uuid8}} convention"
    );

    // worktree subdir exists inside rootdir
    let worktree = rootdir.join("feature-a");
    assert!(
        worktree.is_dir(),
        "worktree subdir should exist: {worktree:?}"
    );

    // sodagun.json exists in rootdir
    let metadata_path = rootdir.join("sodagun.json");
    assert!(
        metadata_path.exists(),
        "sodagun.json should exist in rootdir"
    );

    // sodagun.json is valid and has expected fields
    let raw = std::fs::read_to_string(&metadata_path).unwrap();
    let meta: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(meta["version"], 1);
    assert_eq!(meta["branch"], "feature-a");
    assert!(meta["sandbox_name"].is_null());
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

    let rootdir = PathBuf::from(String::from_utf8(output).unwrap().trim());
    assert_eq!(rootdir.parent().unwrap(), prefix);
    assert!(
        rootdir
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("sodagun_repo_feature-b_")
    );
    assert!(rootdir.join("feature-b").is_dir());
    assert!(rootdir.join("sodagun.json").exists());
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
    let rootdir = PathBuf::from(data["rootdir"].as_str().unwrap());
    assert!(rootdir.is_dir(), "rootdir should exist on disk");
    assert!(
        rootdir.join("feature-c").is_dir(),
        "worktree subdir should exist"
    );
    assert!(
        rootdir.join("sodagun.json").exists(),
        "sodagun.json should exist"
    );
}

#[test]
fn branch_with_slash_sanitized_in_dir() {
    let tmp = TempDir::new().unwrap();
    let repo = make_git_repo(&tmp);
    let wd = workdir(&repo);

    let output = sodagun()
        .args([
            "git",
            "add-worktree",
            wd.to_str().unwrap(),
            "feature/my-thing",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let rootdir = PathBuf::from(String::from_utf8(output).unwrap().trim());
    // '/' in branch name becomes '-' in the worktree directory name
    assert!(rootdir.join("feature-my-thing").is_dir());
    // sodagun.json preserves the original branch name
    let raw = std::fs::read_to_string(rootdir.join("sodagun.json")).unwrap();
    let meta: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(meta["branch"], "feature/my-thing");
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
