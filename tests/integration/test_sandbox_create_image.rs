use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;

use super::utils::sodagun_isolated;

fn config_dir(content: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("sodagun.toml"), content).unwrap();
    tmp
}

// ── CONFIG_NOT_FOUND ──────────────────────────────────────────────────────────

#[test]
fn create_image_config_not_found() {
    let xdg = TempDir::new().unwrap();
    sodagun_isolated(&xdg)
        .args([
            "--project-dir",
            "/nonexistent/rootdir",
            "sandbox",
            "create-image",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_NOT_FOUND"));
}

#[test]
fn create_image_config_not_found_json() {
    let xdg = TempDir::new().unwrap();
    sodagun_isolated(&xdg)
        .args([
            "--output",
            "json",
            "--project-dir",
            "/nonexistent/rootdir",
            "sandbox",
            "create-image",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_NOT_FOUND"));
}

// ── No dockerfile in config ───────────────────────────────────────────────────

#[test]
fn create_image_no_dockerfile_in_config() {
    let xdg = TempDir::new().unwrap();
    let tmp = config_dir("[image]\nbase_image = \"alpine\"\n");
    sodagun_isolated(&xdg)
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "sandbox",
            "create-image",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID").or(predicate::str::contains(
            "no 'dockerfile'",
        )));
}

// ── Dockerfile path not found ─────────────────────────────────────────────────

#[test]
fn create_image_dockerfile_not_exists() {
    let xdg = TempDir::new().unwrap();
    let tmp = config_dir(
        "[image]\ndockerfile = \"./nonexistent.Dockerfile\"\nnamespace_repository = \"org/repo\"\n",
    );
    sodagun_isolated(&xdg)
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "sandbox",
            "create-image",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID"));
}

// ── dockerfile + base_image conflict ─────────────────────────────────────────

#[test]
fn create_image_dockerfile_base_image_conflict() {
    let xdg = TempDir::new().unwrap();
    let tmp = TempDir::new().unwrap();
    // Create the Dockerfile so the path check passes; the conflict check must fire first
    fs::write(tmp.path().join("Dockerfile"), "FROM alpine\n").unwrap();
    let toml = "dockerfile = \"Dockerfile\"\nbase_image = \"alpine\"\nnamespace_repository = \"org/repo\"\n";
    fs::write(
        tmp.path().join("sodagun.toml"),
        format!("[image]\n{toml}"),
    )
    .unwrap();
    sodagun_isolated(&xdg)
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "sandbox",
            "create-image",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID").and(predicate::str::contains(
            "mutually exclusive",
        )));
}

// ── dockerfile + base_snapshot conflict ──────────────────────────────────────

#[test]
fn create_image_dockerfile_base_snapshot_conflict() {
    let xdg = TempDir::new().unwrap();
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("Dockerfile"), "FROM alpine\n").unwrap();
    let toml = "dockerfile = \"Dockerfile\"\nbase_snapshot = \"my-snap\"\nnamespace_repository = \"org/repo\"\n";
    fs::write(
        tmp.path().join("sodagun.toml"),
        format!("[image]\n{toml}"),
    )
    .unwrap();
    sodagun_isolated(&xdg)
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "sandbox",
            "create-image",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("CONFIG_INVALID").and(predicate::str::contains(
            "mutually exclusive",
        )));
}

// ── namespace_repository missing ──────────────────────────────────────────────

#[test]
fn create_image_missing_namespace_repository() {
    let xdg = TempDir::new().unwrap();
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("Dockerfile"), "FROM alpine\n").unwrap();
    fs::write(
        tmp.path().join("sodagun.toml"),
        "[image]\ndockerfile = \"Dockerfile\"\n",
    )
    .unwrap();
    sodagun_isolated(&xdg)
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "sandbox",
            "create-image",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(
            predicate::str::contains("CONFIG_INVALID")
                .and(predicate::str::contains("namespace_repository")),
        );
}

// ── registry.host missing (namespace_repository present) ──────────────────────

#[test]
fn create_image_missing_registry_host() {
    let xdg = TempDir::new().unwrap();
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("Dockerfile"), "FROM alpine\n").unwrap();
    // namespace_repository present, but no registry.host → dockerfile_image_tag should fail
    fs::write(
        tmp.path().join("sodagun.toml"),
        "[image]\ndockerfile = \"Dockerfile\"\nnamespace_repository = \"org/repo\"\n",
    )
    .unwrap();
    sodagun_isolated(&xdg)
        .args([
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "sandbox",
            "create-image",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(
            predicate::str::contains("CONFIG_INVALID")
                .and(predicate::str::contains("registry.host")),
        );
}

// ── JSON output for errors ────────────────────────────────────────────────────

#[test]
fn create_image_missing_registry_host_json() {
    let xdg = TempDir::new().unwrap();
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("Dockerfile"), "FROM alpine\n").unwrap();
    fs::write(
        tmp.path().join("sodagun.toml"),
        "[image]\ndockerfile = \"Dockerfile\"\nnamespace_repository = \"org/repo\"\n",
    )
    .unwrap();
    sodagun_isolated(&xdg)
        .args([
            "--output",
            "json",
            "--project-dir",
            tmp.path().to_str().unwrap(),
            "sandbox",
            "create-image",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::contains("CONFIG_INVALID"))
        .stderr(predicate::str::is_empty());
}
