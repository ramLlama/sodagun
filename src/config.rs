use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::SodagunError;
use crate::util::dashify;

/// Reserved basename for the setup script injected into [`SETUP_ASSETS_DIR`].
/// The leading underscore keeps it from colliding with any user `setup_files`.
pub const SETUP_SCRIPT_NAME: &str = "_setup";

/// A file to inject into `/setup-assets/` during snapshot creation.
#[derive(Debug)]
pub struct SetupFile {
    /// Basename used as `/setup-assets/<name>`.
    pub name: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkMode {
    PublicOnly,
    AllowAll,
    /// No network access. Named to match microsandbox's `NetworkPolicy::none()`,
    /// which `disable_network()` applies.
    #[default]
    None,
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
/// from `setup_script_path`). `setup_files` are resolved at load time.
#[derive(Debug)]
pub struct ImageConfig {
    pub base_image: Option<String>,
    pub base_snapshot: Option<String>,
    /// Resolved setup script content; `None` means no setup, boot base directly.
    pub setup_script: Option<String>,
    /// Files injected into `/setup-assets/` during snapshot creation.
    pub setup_files: Vec<SetupFile>,
    /// Environment variables passed to the ephemeral sandbox during snapshot creation.
    pub env: HashMap<String, String>,
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
        Some(snapshot_name(base, script, &self.setup_files))
    }
}

/// Computes the deterministic snapshot name: `<sanitized_base>_<12 base64url chars of SHA256(script + setup_files)>`.
///
/// Setup files are sorted by name before hashing so the result is order-independent.
pub fn snapshot_name(base: &str, script: &str, setup_files: &[SetupFile]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(script.as_bytes());
    let mut sorted: Vec<_> = setup_files.iter().collect();
    sorted.sort_by_key(|f| &f.name);
    for f in sorted {
        hasher.update(f.name.as_bytes());
        hasher.update(&f.content);
    }
    let hash = hasher.finalize();
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&hash[..]);
    let prefix = &b64[..12];
    format!("{}_{prefix}", dashify(base))
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
    /// Paths relative to the config file; resolved to `SetupFile`s in `validate_image_config`.
    setup_files: Option<Vec<String>>,
    #[serde(default)]
    env: HashMap<String, String>,
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

/// Returns the default [`SandboxConfig`] used when no `sodagun.toml` is present.
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

/// Returns the default [`ImageConfig`] used when no `sodagun.toml` is present:
/// alpine:latest with no setup script.
pub fn default_image_config() -> ImageConfig {
    ImageConfig {
        base_image: Some("alpine:latest".to_string()),
        base_snapshot: None,
        setup_script: None,
        setup_files: Vec::new(),
        env: HashMap::new(),
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

    // [image] is required when sodagun.toml exists
    let raw_image = file.image.ok_or_else(|| SodagunError {
        code: "CONFIG_INVALID",
        message: "[image] section is required in sodagun.toml".to_string(),
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

    // Resolve setup_files paths → SetupFile { name, content }
    let config_dir = config_path.parent().unwrap_or(Path::new("."));
    let mut setup_files = Vec::new();
    for path_str in raw.setup_files.unwrap_or_default() {
        let abs = config_dir.join(&path_str);
        let name = abs
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| SodagunError {
                code: "CONFIG_INVALID",
                message: format!("setup_files entry '{path_str}' has a non-UTF-8 basename"),
            })?
            .to_string();
        // The setup script is injected under this same basename; a collision would
        // overwrite one with the other.
        if name == SETUP_SCRIPT_NAME {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: format!(
                    "setup_files entry '{path_str}' uses reserved name '{SETUP_SCRIPT_NAME}'"
                ),
            });
        }
        let content = std::fs::read(&abs).map_err(|e| SodagunError {
            code: "CONFIG_INVALID",
            message: format!("failed to read setup_files entry '{}': {e}", abs.display()),
        })?;
        setup_files.push(SetupFile { name, content });
    }

    Ok(ImageConfig {
        base_image: raw.base_image,
        base_snapshot: raw.base_snapshot,
        setup_script,
        setup_files,
        env: raw.env,
    })
}

/// Parse a Docker-style volume string (`"host:guest"` or `"host:guest:ro"`).
///
/// A leading `~` in the host path is expanded to `$HOME` here, at launch time,
/// rather than at config-parse time — so the parsed [`SandboxConfig`] never
/// carries an environment-dependent absolute path.
pub fn parse_volume(s: &str) -> Result<(PathBuf, String, bool), SodagunError> {
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    if parts.len() < 2 {
        return Err(SodagunError {
            code: "CONFIG_INVALID",
            message: format!("volume '{s}' must be 'host:guest' or 'host:guest:ro'"),
        });
    }
    let host_raw = parts[0];
    let guest = parts[1].to_string();
    let readonly = parts.get(2).is_some_and(|f| *f == "ro");

    let host = if let Some(rest) = host_raw.strip_prefix('~') {
        let home = std::env::var("HOME").map_err(|_| SodagunError {
            code: "CONFIG_INVALID",
            message: "cannot expand '~' in volume: $HOME is not set".to_string(),
        })?;
        PathBuf::from(format!("{home}{rest}"))
    } else {
        PathBuf::from(host_raw)
    };

    Ok((host, guest, readonly))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;
    use tempfile::{NamedTempFile, TempDir};

    // Serialize tests that mutate $HOME so they don't race parse_volume_tilde_expansion.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

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
    fn valid_image_env() {
        let f = write_config(
            r#"
[image]
base_image = "alpine"

[image.env]
HOME = "/root"
CUSTOM = "value"
"#,
        );
        let (img, _) = load_config(f.path()).unwrap();
        assert_eq!(img.env.get("HOME").map(String::as_str), Some("/root"));
        assert_eq!(img.env.get("CUSTOM").map(String::as_str), Some("value"));
    }

    #[test]
    fn valid_image_env_defaults_empty() {
        let f = write_config("[image]\nbase_image = \"alpine\"\n");
        let (img, _) = load_config(f.path()).unwrap();
        assert!(img.env.is_empty());
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
        let err = load_config(Path::new("/nonexistent/sodagun.toml")).unwrap_err();
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
        assert_eq!(sb.network.mode, NetworkMode::None);
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
        let a = snapshot_name("alpine:latest", "#!/bin/sh\napk add git\n", &[]);
        let b = snapshot_name("alpine:latest", "#!/bin/sh\napk add git\n", &[]);
        assert_eq!(a, b);
    }

    #[test]
    fn snapshot_name_changes_with_script() {
        let a = snapshot_name("alpine:latest", "#!/bin/sh\napk add git\n", &[]);
        let b = snapshot_name("alpine:latest", "#!/bin/sh\napk add curl\n", &[]);
        assert_ne!(a, b);
    }

    #[test]
    fn snapshot_name_sanitizes_image() {
        let name = snapshot_name("alpine:latest", "#!/bin/sh\n", &[]);
        assert!(name.starts_with("alpine-latest_"));
    }

    #[test]
    fn snapshot_name_sanitizes_slash_and_at() {
        let name = snapshot_name("ghcr.io/foo/bar:v1", "#!/bin/sh\n", &[]);
        assert!(name.starts_with("ghcr.io-foo-bar-v1_"));
    }

    #[test]
    fn snapshot_name_hash_length() {
        let name = snapshot_name("alpine:latest", "#!/bin/sh\n", &[]);
        // format is "<sanitized>_<12chars>"
        let hash_part = name.rsplit_once('_').unwrap().1;
        assert_eq!(hash_part.len(), 12);
    }

    #[test]
    fn snapshot_name_changes_with_setup_file_content() {
        let files_a = vec![SetupFile {
            name: "rust-toolchain.toml".to_string(),
            content: b"[toolchain]\nchannel = \"1.85\"".to_vec(),
        }];
        let files_b = vec![SetupFile {
            name: "rust-toolchain.toml".to_string(),
            content: b"[toolchain]\nchannel = \"1.86\"".to_vec(),
        }];
        let a = snapshot_name("alpine:latest", "#!/bin/sh\n", &files_a);
        let b = snapshot_name("alpine:latest", "#!/bin/sh\n", &files_b);
        assert_ne!(a, b);
    }

    #[test]
    fn snapshot_name_stable_with_same_setup_files() {
        let files = vec![
            SetupFile {
                name: "Cargo.lock".to_string(),
                content: b"[lock-file]".to_vec(),
            },
            SetupFile {
                name: "rust-toolchain.toml".to_string(),
                content: b"[toolchain]".to_vec(),
            },
        ];
        let a = snapshot_name("alpine:latest", "#!/bin/sh\n", &files);
        let b = snapshot_name("alpine:latest", "#!/bin/sh\n", &files);
        assert_eq!(a, b);
    }

    #[test]
    fn setup_files_parsed_and_resolved() {
        use std::io::Write as _;
        let tmp = TempDir::new().unwrap();
        let asset = tmp.path().join("rust-toolchain.toml");
        std::fs::write(&asset, "[toolchain]\nchannel = \"stable\"").unwrap();
        let config_content = "[image]\nbase_image = \"alpine\"\nsetup_script = \"#!/bin/sh\\n\"\nsetup_files = [\"rust-toolchain.toml\"]\n".to_string();
        let mut cfg = NamedTempFile::new_in(tmp.path()).unwrap();
        cfg.write_all(config_content.as_bytes()).unwrap();
        let (img, _) = load_config(cfg.path()).unwrap();
        assert_eq!(img.setup_files.len(), 1);
        assert_eq!(img.setup_files[0].name, "rust-toolchain.toml");
        assert!(img.setup_files[0].content.starts_with(b"[toolchain]"));
    }

    #[test]
    fn setup_files_missing_file_returns_config_invalid() {
        let f = write_config(
            "[image]\nbase_image = \"alpine\"\nsetup_script = \"#!/bin/sh\\n\"\nsetup_files = [\"nonexistent.toml\"]\n",
        );
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn setup_files_reserved_name_returns_config_invalid() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(SETUP_SCRIPT_NAME), "x").unwrap();
        let mut cfg = NamedTempFile::new_in(tmp.path()).unwrap();
        cfg.write_all(
            format!(
                "[image]\nbase_image = \"alpine\"\nsetup_script = \"#!/bin/sh\\n\"\nsetup_files = [\"{SETUP_SCRIPT_NAME}\"]\n"
            )
            .as_bytes(),
        )
        .unwrap();
        let err = load_config(cfg.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    // ── parse_volume tests ────────────────────────────────────────────────────

    #[test]
    fn parse_volume_basic() {
        let (host, guest, ro) = parse_volume("/host/path:/guest/path").unwrap();
        assert_eq!(host, PathBuf::from("/host/path"));
        assert_eq!(guest, "/guest/path");
        assert!(!ro);
    }

    #[test]
    fn parse_volume_readonly() {
        let (_, _, ro) = parse_volume("/host:/guest:ro").unwrap();
        assert!(ro);
    }

    #[test]
    fn parse_volume_tilde_expansion() {
        let _guard = HOME_LOCK.lock().unwrap();
        if let Ok(home) = std::env::var("HOME") {
            let (host, _, _) = parse_volume("~/.config/claude:/root/.config/claude:ro").unwrap();
            assert_eq!(host, PathBuf::from(format!("{home}/.config/claude")));
        }
    }

    #[test]
    fn parse_volume_no_home_error() {
        let _guard = HOME_LOCK.lock().unwrap();
        let saved = std::env::var("HOME").ok();
        unsafe { std::env::remove_var("HOME") };
        let err = parse_volume("~/.config:/root/.config").unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
        if let Some(h) = saved {
            unsafe { std::env::set_var("HOME", h) };
        }
    }

    #[test]
    fn parse_volume_missing_guest_error() {
        let err = parse_volume("/only-host").unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }
}
