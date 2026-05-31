use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::SodagunError;

const METADATA_FILE: &str = "sodagun.json";
const WORKSPACE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceMetadata {
    pub version: u32,
    pub repo_path: PathBuf,
    pub branch: String,
    /// ISO 8601 UTC timestamp, e.g. "2026-05-30T12:00:00Z"
    pub created_at: String,
    pub worktree_path: PathBuf,
    pub sandbox_name: Option<String>,
}

impl WorkspaceMetadata {
    /// Construct a fresh workspace record. Sets version and created_at automatically.
    pub fn new(repo_path: PathBuf, branch: String, worktree_path: PathBuf) -> Self {
        Self {
            version: WORKSPACE_VERSION,
            repo_path,
            branch,
            created_at: now_iso8601(),
            worktree_path,
            sandbox_name: None,
        }
    }

    /// Serialize and write to `<rootdir>/sodagun.json`.
    pub fn write(&self, rootdir: &Path) -> Result<(), SodagunError> {
        let path = rootdir.join(METADATA_FILE);
        let json = serde_json::to_string_pretty(self).map_err(|e| SodagunError {
            code: "WORKSPACE_INVALID",
            message: format!("failed to serialize workspace metadata: {e}"),
        })?;
        std::fs::write(&path, json).map_err(|e| SodagunError {
            code: "WORKSPACE_INVALID",
            message: format!("failed to write {}: {e}", path.display()),
        })
    }

    /// Read and deserialize `<rootdir>/sodagun.json`.
    /// Returns WORKSPACE_NOT_FOUND if the file is absent, WORKSPACE_INVALID if malformed.
    pub fn read(rootdir: &Path) -> Result<WorkspaceMetadata, SodagunError> {
        let path = rootdir.join(METADATA_FILE);
        if !path.exists() {
            return Err(SodagunError {
                code: "WORKSPACE_NOT_FOUND",
                message: format!(
                    "no sodagun.json found in {}; was this rootdir created by sodagun?",
                    rootdir.display()
                ),
            });
        }
        let contents = std::fs::read_to_string(&path).map_err(|e| SodagunError {
            code: "WORKSPACE_INVALID",
            message: format!("failed to read {}: {e}", path.display()),
        })?;
        serde_json::from_str(&contents).map_err(|e| SodagunError {
            code: "WORKSPACE_INVALID",
            message: format!("malformed sodagun.json in {}: {e}", rootdir.display()),
        })
    }

    /// Read sodagun.json, update sandbox_name, and rewrite.
    pub fn set_sandbox_name(rootdir: &Path, name: Option<String>) -> Result<(), SodagunError> {
        let mut meta = Self::read(rootdir)?;
        meta.sandbox_name = name;
        meta.write(rootdir)
    }
}

/// Current UTC time as an ISO 8601 timestamp (YYYY-MM-DDTHH:MM:SSZ).
pub fn now_iso8601() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_iso8601_format() {
        let ts = now_iso8601();
        // YYYY-MM-DDTHH:MM:SSZ is exactly 20 characters
        assert_eq!(ts.len(), 20, "timestamp={ts}");
        assert!(ts.ends_with('Z'), "timestamp={ts}");
        let year: i32 = ts[0..4].parse().unwrap();
        assert!(year >= 2025, "year should be recent: {year}");
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let meta = WorkspaceMetadata::new(
            PathBuf::from("/repo/path"),
            "feature/my-thing".to_string(),
            PathBuf::from("/root/feature-my-thing"),
        );
        meta.write(dir.path()).unwrap();
        let read_back = WorkspaceMetadata::read(dir.path()).unwrap();
        assert_eq!(read_back.version, WORKSPACE_VERSION);
        assert_eq!(read_back.branch, "feature/my-thing");
        assert!(read_back.sandbox_name.is_none());
    }

    #[test]
    fn read_missing_returns_workspace_not_found() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = WorkspaceMetadata::read(dir.path()).unwrap_err();
        assert_eq!(err.code, "WORKSPACE_NOT_FOUND");
    }

    #[test]
    fn set_sandbox_name_updates_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let meta = WorkspaceMetadata::new(
            PathBuf::from("/repo"),
            "main".to_string(),
            PathBuf::from("/root/main"),
        );
        meta.write(dir.path()).unwrap();
        WorkspaceMetadata::set_sandbox_name(dir.path(), Some("sodagun-main-abc123".to_string()))
            .unwrap();
        let updated = WorkspaceMetadata::read(dir.path()).unwrap();
        assert_eq!(updated.sandbox_name.as_deref(), Some("sodagun-main-abc123"));
    }
}
