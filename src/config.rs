use std::collections::HashMap;
use std::path::Path;

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::SodagunError;

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkMode {
    PublicOnly,
    AllowAll,
    #[default]
    Airgapped,
}

#[derive(Debug, Deserialize, Default)]
pub struct NetworkConfig {
    #[serde(default)]
    pub mode: NetworkMode,
}

#[derive(Debug, Deserialize)]
pub struct SecretConfig {
    pub value_from_env: Option<String>,
    pub value: Option<String>,
    pub allowed_hosts: Vec<String>,
}

/// Resolved image/snapshot configuration from the `[image]` TOML table.
///
/// `setup_script` is always the resolved script content (either inline or read
/// from `setup_script_path`). `setup_script_path` is preserved for error messages.
#[derive(Debug)]
pub struct ImageConfig {
    pub base_image: Option<String>,
    pub base_snapshot: Option<String>,
    /// Resolved setup script content; `None` means no setup, boot base directly.
    pub setup_script: Option<String>,
}

impl ImageConfig {
    /// Returns the deterministic snapshot name for this config, or `None` if
    /// no setup script is configured.
    pub fn derived_snapshot_name(&self) -> Option<String> {
        let script = self.setup_script.as_ref()?;
        let base = self
            .base_image
            .as_deref()
            .or(self.base_snapshot.as_deref())
            .unwrap_or("");
        Some(snapshot_name(base, script))
    }
}

/// Computes the deterministic snapshot name: `<sanitized_base>_<12 base64url chars of SHA256(script)>`.
pub fn snapshot_name(base: &str, script: &str) -> String {
    let hash = Sha256::digest(script.as_bytes());
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&hash[..]);
    let prefix = &b64[..12];
    let sanitized = base.replace([':', '/', '@'], "-");
    format!("{sanitized}_{prefix}")
}

#[derive(Debug, Deserialize)]
pub struct SandboxConfig {
    #[serde(default = "default_working_dir")]
    pub working_dir: String,
    #[serde(default = "default_memory_mb")]
    pub memory_mb: u32,
    #[serde(default = "default_cpus")]
    pub cpus: u8,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub secrets: HashMap<String, SecretConfig>,
}

/// Raw deserialization struct for `[image]` — before validation / file resolution.
#[derive(Deserialize)]
struct RawImageConfig {
    base_image: Option<String>,
    base_snapshot: Option<String>,
    setup_script: Option<String>,
    setup_script_path: Option<String>,
}

fn default_working_dir() -> String {
    "/workspace".to_string()
}
fn default_memory_mb() -> u32 {
    512
}
fn default_cpus() -> u8 {
    1
}

/// Top-level wrapper matching the TOML file structure.
#[derive(Deserialize)]
struct ConfigFile {
    image: Option<RawImageConfig>,
    sandbox: Option<SandboxConfig>,
}

/// Returns the default [`SandboxConfig`] used when no `.sodagun.toml` is present.
pub fn default_sandbox_config() -> SandboxConfig {
    SandboxConfig {
        working_dir: default_working_dir(),
        memory_mb: default_memory_mb(),
        cpus: default_cpus(),
        volumes: Vec::new(),
        network: NetworkConfig::default(),
        env: std::collections::HashMap::new(),
        secrets: std::collections::HashMap::new(),
    }
}

/// Returns the default [`ImageConfig`] used when no `.sodagun.toml` is present:
/// alpine:latest with no setup script.
pub fn default_image_config() -> ImageConfig {
    ImageConfig {
        base_image: Some("alpine:latest".to_string()),
        base_snapshot: None,
        setup_script: None,
    }
}

/// Load and validate both `[image]` and `[sandbox]` from `path`.
///
/// Returns `CONFIG_NOT_FOUND` if the file is missing, `CONFIG_INVALID` for any
/// parse or validation failure.
pub fn load_config(path: &Path) -> Result<(ImageConfig, SandboxConfig), SodagunError> {
    if !path.exists() {
        return Err(SodagunError {
            code: "CONFIG_NOT_FOUND",
            message: format!("no config file at {}", path.display()),
        });
    }

    let contents = std::fs::read_to_string(path).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("failed to read config: {e}"),
    })?;

    let file: ConfigFile = toml::from_str(&contents).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("invalid TOML: {e}"),
    })?;

    // [image] is required when .sodagun.toml exists
    let raw_image = file.image.ok_or_else(|| SodagunError {
        code: "CONFIG_INVALID",
        message: "[image] section is required in .sodagun.toml".to_string(),
    })?;

    let sandbox = file.sandbox.unwrap_or_else(default_sandbox_config);

    let image = validate_image_config(raw_image, path)?;

    // Env var names must not appear in both env and secrets.
    for key in sandbox.secrets.keys() {
        if sandbox.env.contains_key(key.as_str()) {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: format!("'{key}' appears in both [sandbox.env] and [sandbox.secrets]"),
            });
        }
    }

    Ok((image, sandbox))
}

/// Load only the `[image]` section from `path` (used by `snapshot create`).
pub fn load_image_config(path: &Path) -> Result<ImageConfig, SodagunError> {
    let (image, _sandbox) = load_config(path)?;
    Ok(image)
}

fn validate_image_config(
    raw: RawImageConfig,
    config_path: &Path,
) -> Result<ImageConfig, SodagunError> {
    // Exactly one of base_image / base_snapshot
    match (&raw.base_image, &raw.base_snapshot) {
        (None, None) => {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: "one of 'base_image' or 'base_snapshot' is required in [image]"
                    .to_string(),
            });
        }
        (Some(_), Some(_)) => {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: "'base_image' and 'base_snapshot' are mutually exclusive in [image]"
                    .to_string(),
            });
        }
        _ => {}
    }

    // At most one of setup_script / setup_script_path
    if raw.setup_script.is_some() && raw.setup_script_path.is_some() {
        return Err(SodagunError {
            code: "CONFIG_INVALID",
            message: "'setup_script' and 'setup_script_path' are mutually exclusive in [image]"
                .to_string(),
        });
    }

    // Resolve setup_script_path → script content
    let setup_script = if let Some(ref script_path) = raw.setup_script_path {
        let abs = config_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(script_path);
        let content = std::fs::read_to_string(&abs).map_err(|e| SodagunError {
            code: "CONFIG_INVALID",
            message: format!("failed to read setup_script_path '{}': {e}", abs.display()),
        })?;
        Some(content)
    } else {
        raw.setup_script
    };

    Ok(ImageConfig {
        base_image: raw.base_image,
        base_snapshot: raw.base_snapshot,
        setup_script,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_config(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    // ── [image] section tests ─────────────────────────────────────────────────

    #[test]
    fn valid_image_base_image_only() {
        let f = write_config(
            r#"
[image]
base_image = "alpine:latest"
"#,
        );
        let (img, _) = load_config(f.path()).unwrap();
        assert_eq!(img.base_image.as_deref(), Some("alpine:latest"));
        assert!(img.base_snapshot.is_none());
        assert!(img.setup_script.is_none());
        assert!(img.derived_snapshot_name().is_none());
    }

    #[test]
    fn valid_image_base_snapshot_only() {
        let f = write_config(
            r#"
[image]
base_snapshot = "my-snap"
"#,
        );
        let (img, _) = load_config(f.path()).unwrap();
        assert!(img.base_image.is_none());
        assert_eq!(img.base_snapshot.as_deref(), Some("my-snap"));
    }

    #[test]
    fn valid_image_with_inline_script() {
        let f = write_config(
            r##"
[image]
base_image = "debian"
setup_script = "#!/bin/bash\napt-get update\n"
"##,
        );
        let (img, _) = load_config(f.path()).unwrap();
        assert!(img.setup_script.is_some());
        assert!(img.derived_snapshot_name().is_some());
    }

    #[test]
    fn valid_image_with_script_path() {
        use std::io::Write as _;
        let script_file = NamedTempFile::new().unwrap();
        writeln!(script_file.as_file(), "#!/bin/bash\necho hello").unwrap();

        let config_content = format!(
            "[image]\nbase_image = \"alpine:latest\"\nsetup_script_path = \"{}\"\n",
            script_file.path().display()
        );
        // write_config uses NamedTempFile in the same dir; use explicit path
        let mut cfg = NamedTempFile::new().unwrap();
        cfg.write_all(config_content.as_bytes()).unwrap();
        let (img, _) = load_config(cfg.path()).unwrap();
        assert!(img.setup_script.as_deref().unwrap().contains("echo hello"));
    }

    #[test]
    fn error_image_neither_base() {
        let f = write_config("[image]\nmemory_mb = 512\n");
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_image_both_base() {
        let f = write_config("[image]\nbase_image = \"alpine\"\nbase_snapshot = \"snap\"\n");
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_image_both_scripts() {
        let f = write_config(
            "[image]\nbase_image = \"alpine\"\nsetup_script = \"#!/bin/sh\"\nsetup_script_path = \"./s.sh\"\n",
        );
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_image_missing_script_file() {
        let f = write_config(
            "[image]\nbase_image = \"alpine\"\nsetup_script_path = \"/nonexistent/setup.sh\"\n",
        );
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_missing_image_section() {
        let f = write_config("[sandbox]\nworking_dir = \"/workspace\"\n");
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_config_not_found() {
        let err = load_config(Path::new("/nonexistent/.sodagun.toml")).unwrap_err();
        assert_eq!(err.code, "CONFIG_NOT_FOUND");
    }

    #[test]
    fn error_invalid_toml() {
        let f = write_config("not valid toml @@@@");
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    // ── [sandbox] section tests ───────────────────────────────────────────────

    #[test]
    fn valid_sandbox_defaults() {
        let f = write_config("[image]\nbase_image = \"debian\"\n");
        let (_, sb) = load_config(f.path()).unwrap();
        assert_eq!(sb.working_dir, "/workspace");
        assert_eq!(sb.memory_mb, 512);
        assert_eq!(sb.cpus, 1);
        assert_eq!(sb.network.mode, NetworkMode::Airgapped);
    }

    #[test]
    fn valid_sandbox_full() {
        let f = write_config(
            r#"
[image]
base_image = "debian"

[sandbox]
working_dir = "/app"
memory_mb = 1024
cpus = 2
volumes = ["~/.config/claude:/root/.config/claude:ro"]

[sandbox.network]
mode = "public-only"

[sandbox.env]
TERM = "xterm-256color"

[sandbox.secrets.ANTHROPIC_API_KEY]
value_from_env = "ANTHROPIC_API_KEY"
allowed_hosts = ["api.anthropic.com"]
"#,
        );
        let (_, sb) = load_config(f.path()).unwrap();
        assert_eq!(sb.memory_mb, 1024);
        assert_eq!(sb.cpus, 2);
        assert_eq!(sb.network.mode, NetworkMode::PublicOnly);
        assert_eq!(sb.volumes, ["~/.config/claude:/root/.config/claude:ro"]);
        assert_eq!(
            sb.env.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
        let secret = sb.secrets.get("ANTHROPIC_API_KEY").unwrap();
        assert_eq!(secret.value_from_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn error_unknown_network_mode() {
        let f = write_config(
            "[image]\nbase_image = \"debian\"\n[sandbox.network]\nmode = \"unrestricted\"\n",
        );
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_env_secret_conflict() {
        let f = write_config(
            r#"
[image]
base_image = "debian"

[sandbox.env]
ANTHROPIC_API_KEY = "literal"

[sandbox.secrets.ANTHROPIC_API_KEY]
value_from_env = "ANTHROPIC_API_KEY"
allowed_hosts = ["api.anthropic.com"]
"#,
        );
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    // ── snapshot_name tests ───────────────────────────────────────────────────

    #[test]
    fn snapshot_name_deterministic() {
        let a = snapshot_name("alpine:latest", "#!/bin/sh\napk add git\n");
        let b = snapshot_name("alpine:latest", "#!/bin/sh\napk add git\n");
        assert_eq!(a, b);
    }

    #[test]
    fn snapshot_name_changes_with_script() {
        let a = snapshot_name("alpine:latest", "#!/bin/sh\napk add git\n");
        let b = snapshot_name("alpine:latest", "#!/bin/sh\napk add curl\n");
        assert_ne!(a, b);
    }

    #[test]
    fn snapshot_name_sanitizes_image() {
        let name = snapshot_name("alpine:latest", "#!/bin/sh\n");
        assert!(name.starts_with("alpine-latest_"));
    }

    #[test]
    fn snapshot_name_sanitizes_slash_and_at() {
        let name = snapshot_name("ghcr.io/foo/bar:v1", "#!/bin/sh\n");
        assert!(name.starts_with("ghcr.io-foo-bar-v1_"));
    }

    #[test]
    fn snapshot_name_hash_length() {
        let name = snapshot_name("alpine:latest", "#!/bin/sh\n");
        // format is "<sanitized>_<12chars>"
        let hash_part = name.rsplit_once('_').unwrap().1;
        assert_eq!(hash_part.len(), 12);
    }
}
