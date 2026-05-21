use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

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

#[derive(Debug, Deserialize)]
pub struct SandboxConfig {
    pub image: Option<String>,
    pub snapshot: Option<String>,
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

fn default_image() -> String {
    "alpine:latest".to_string()
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

/// Top-level wrapper matching the `[sandbox]` TOML table.
#[derive(Deserialize)]
struct ConfigFile {
    sandbox: SandboxConfig,
}

/// Returns the default [`SandboxConfig`] used when no `.sodagun.toml` is present.
pub fn default_config() -> SandboxConfig {
    SandboxConfig {
        image: Some(default_image()),
        snapshot: None,
        working_dir: default_working_dir(),
        memory_mb: default_memory_mb(),
        cpus: default_cpus(),
        volumes: Vec::new(),
        network: NetworkConfig::default(),
        env: std::collections::HashMap::new(),
        secrets: std::collections::HashMap::new(),
    }
}

/// Load and validate `.sodagun.toml` from `path`.
///
/// Returns `CONFIG_NOT_FOUND` if the file is missing, `CONFIG_INVALID` for any
/// parse or validation failure.
pub fn load_config(path: &Path) -> Result<SandboxConfig, SodagunError> {
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

    let config = file.sandbox;

    // Exactly one of image / snapshot must be set.
    match (&config.image, &config.snapshot) {
        (None, None) => {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: "one of 'image' or 'snapshot' is required".to_string(),
            });
        }
        (Some(_), Some(_)) => {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: "'image' and 'snapshot' are mutually exclusive".to_string(),
            });
        }
        _ => {}
    }

    // Env var names must not appear in both env and secrets.
    for key in config.secrets.keys() {
        if config.env.contains_key(key.as_str()) {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: format!("'{key}' appears in both [sandbox.env] and [sandbox.secrets]"),
            });
        }
    }

    Ok(config)
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

    #[test]
    fn valid_config_image() {
        let f = write_config(
            r#"
[sandbox]
image = "debian"
"#,
        );
        let config = load_config(f.path()).unwrap();
        assert_eq!(config.image.as_deref(), Some("debian"));
        assert!(config.snapshot.is_none());
        assert_eq!(config.working_dir, "/workspace");
        assert_eq!(config.memory_mb, 512);
        assert_eq!(config.cpus, 1);
        assert_eq!(config.network.mode, NetworkMode::Airgapped);
    }

    #[test]
    fn valid_config_snapshot() {
        let f = write_config(
            r#"
[sandbox]
snapshot = "my-snapshot"
"#,
        );
        let config = load_config(f.path()).unwrap();
        assert!(config.image.is_none());
        assert_eq!(config.snapshot.as_deref(), Some("my-snapshot"));
    }

    #[test]
    fn valid_config_full() {
        let f = write_config(
            r#"
[sandbox]
image = "debian"
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
        let config = load_config(f.path()).unwrap();
        assert_eq!(config.memory_mb, 1024);
        assert_eq!(config.cpus, 2);
        assert_eq!(config.network.mode, NetworkMode::PublicOnly);
        assert_eq!(config.volumes, ["~/.config/claude:/root/.config/claude:ro"]);
        assert_eq!(
            config.env.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
        let secret = config.secrets.get("ANTHROPIC_API_KEY").unwrap();
        assert_eq!(secret.value_from_env.as_deref(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(secret.allowed_hosts, ["api.anthropic.com"]);
    }

    #[test]
    fn error_neither_image_nor_snapshot() {
        let f = write_config(
            r#"
[sandbox]
memory_mb = 512
"#,
        );
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_both_image_and_snapshot() {
        let f = write_config(
            r#"
[sandbox]
image = "debian"
snapshot = "my-snapshot"
"#,
        );
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_invalid_toml() {
        let f = write_config("not valid toml @@@@");
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_unknown_network_mode() {
        let f = write_config(
            r#"
[sandbox]
image = "debian"

[sandbox.network]
mode = "unrestricted"
"#,
        );
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_env_secret_conflict() {
        let f = write_config(
            r#"
[sandbox]
image = "debian"

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

    #[test]
    fn error_config_not_found() {
        let err = load_config(Path::new("/nonexistent/.sodagun.toml")).unwrap_err();
        assert_eq!(err.code, "CONFIG_NOT_FOUND");
    }

    #[test]
    fn volume_tilde_preserved_at_parse_time() {
        let f = write_config(
            r#"
[sandbox]
image = "debian"
volumes = ["~/.config/claude:/root/.config/claude:ro"]
"#,
        );
        let config = load_config(f.path()).unwrap();
        // Tilde is NOT expanded at parse time; expansion happens at launch.
        assert_eq!(
            config.volumes[0],
            "~/.config/claude:/root/.config/claude:ro"
        );
    }
}
