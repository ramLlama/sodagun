use std::process::Command;

fn main() {
    // Rerun when the git HEAD or index changes (commit, checkout, staging).
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());

    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let build_version = match sha {
        Some(sha) if dirty => format!("{pkg_version}+{sha}-dirty"),
        Some(sha) => format!("{pkg_version}+{sha}"),
        None => pkg_version,
    };

    println!("cargo:rustc-env=SODAGUN_BUILD_VERSION={build_version}");
}
