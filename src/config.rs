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

/// Built-in network policy names. Always available; `network-policies.toml` cannot redefine them.
pub const RESERVED_POLICY_NAMES: &[&str] = &["none", "allow-all", "public-only"];

// ── Network config types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigAction {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigDirection {
    Egress,
    Ingress,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkRule {
    pub direction: ConfigDirection,
    pub action: ConfigAction,
    pub destination: String,
    pub protocol: Option<ConfigProtocol>,
    #[serde(default)]
    pub ports: Vec<u16>,
}

/// Network configuration for a sandbox; used in both `RawSandboxConfig` and `SandboxConfig`.
///
/// `#[serde(deny_unknown_fields)]` ensures the old `mode` field is rejected at parse time.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    pub policy: Option<String>,
    pub default_egress: Option<ConfigAction>,
    pub default_ingress: Option<ConfigAction>,
    #[serde(default)]
    pub rules: Vec<NetworkRule>,
}

// ── Env / secret value sources ────────────────────────────────────────────────

/// Dynamic value source shared by `[sandbox.env]` and `[sandbox.secrets]`.
///
/// Exactly one of the three fields must be set; the constraint is enforced at
/// launch time (not parse time) so error messages are cleaner.
#[derive(Debug, Clone, Deserialize)]
pub struct ValueSource {
    pub value: Option<String>,
    pub value_from_env: Option<String>,
    /// Shell command whose stdout (trimmed) is the value. Runs on the host at launch time.
    pub value_from_cmd: Option<String>,
}

/// Value for a `[sandbox.env]` entry — either a plain string literal or a dynamic source.
///
/// ```toml
/// [sandbox.env]
/// TERM = "xterm-256color"           # Literal
///
/// [sandbox.env.MY_TOKEN]
/// value_from_cmd = "get-token.sh"   # Dynamic
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EnvValue {
    Literal(String),
    Dynamic(ValueSource),
}

#[derive(Debug, Deserialize)]
pub struct SecretConfig {
    pub value_from_env: Option<String>,
    pub value: Option<String>,
    pub value_from_cmd: Option<String>,
    pub allowed_hosts: Vec<String>,
}

// ── Raw sandbox config (for deserialization and merging) ──────────────────────

/// Sandbox config as deserialized from TOML — all scalars are `Option` so that
/// absent fields can be distinguished from default values during config merging.
#[derive(Debug, Default, Deserialize)]
pub struct RawSandboxConfig {
    pub working_dir: Option<String>,
    pub memory_mb: Option<u32>,
    pub cpus: Option<u8>,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub env: HashMap<String, EnvValue>,
    #[serde(default)]
    pub secrets: HashMap<String, SecretConfig>,
}

// ── Resolved sandbox config (fully merged) ───────────────────────────────────

/// Fully resolved sandbox configuration after merging user + project configs.
/// Not `Deserialize` — always produced by [`merge_sandbox_configs`].
#[derive(Debug)]
pub struct SandboxConfig {
    pub working_dir: String,
    pub memory_mb: u32,
    pub cpus: u8,
    pub volumes: Vec<String>,
    pub network: NetworkConfig,
    pub env: HashMap<String, EnvValue>,
    pub secrets: HashMap<String, SecretConfig>,
}

// ── Named network policy (from ~/.config/sodagun/network-policies.toml) ──────

#[derive(Debug, Clone, Deserialize)]
pub struct NamedPolicy {
    pub default_egress: Option<ConfigAction>,
    pub default_ingress: Option<ConfigAction>,
    #[serde(default)]
    pub rules: Vec<NetworkRule>,
}

// ── Image config ──────────────────────────────────────────────────────────────

/// A file to inject into `/setup-assets/` during snapshot creation.
#[derive(Debug)]
pub struct SetupFile {
    /// Basename used as `/setup-assets/<name>`.
    pub name: String,
    pub content: Vec<u8>,
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

// ── Default helpers ───────────────────────────────────────────────────────────

fn default_working_dir() -> String {
    "/workspace".to_string()
}
fn default_memory_mb() -> u32 {
    512
}
fn default_cpus() -> u8 {
    1
}

// ── Returns the default image config when no sodagun.toml is present ─────────

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

// ── TOML deserialization ──────────────────────────────────────────────────────

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

/// Top-level wrapper matching the TOML file structure.
#[derive(Deserialize)]
struct ConfigFile {
    image: Option<RawImageConfig>,
    sandbox: Option<RawSandboxConfig>,
}

/// Minimal wrapper for the user-level config file; `[image]` is silently ignored.
#[derive(Deserialize)]
struct UserConfigFile {
    sandbox: Option<RawSandboxConfig>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load and validate both `[image]` and `[sandbox]` from `path`.
///
/// Returns `CONFIG_NOT_FOUND` if the file is missing, `CONFIG_INVALID` for any
/// parse or validation failure. The returned `RawSandboxConfig` requires merging
/// with the user config via [`merge_sandbox_configs`] before use.
pub fn load_config(path: &Path) -> Result<(ImageConfig, RawSandboxConfig), SodagunError> {
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

    let sandbox = file.sandbox.unwrap_or_default();
    let image = validate_image_config(raw_image, path)?;

    Ok((image, sandbox))
}

/// Load only the `[image]` section from `path` (used by `snapshot create`).
pub fn load_image_config(path: &Path) -> Result<ImageConfig, SodagunError> {
    let (image, _sandbox) = load_config(path)?;
    Ok(image)
}

/// Load the `[sandbox]` section from the user-level config file
/// (`$XDG_CONFIG_HOME/sodagun/sodagun.toml`).
///
/// Returns `None` if the file does not exist. Returns `CONFIG_INVALID` if the
/// file exists but contains invalid TOML.
pub fn load_user_sandbox_config() -> Result<Option<RawSandboxConfig>, SodagunError> {
    let Some(path) = config_path("sodagun.toml") else {
        return Ok(None);
    };
    load_user_sandbox_config_from_path(&path)
}

pub(crate) fn load_user_sandbox_config_from_path(
    path: &Path,
) -> Result<Option<RawSandboxConfig>, SodagunError> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("failed to read user config: {e}"),
    })?;
    let file: UserConfigFile = toml::from_str(&contents).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("invalid TOML in user config: {e}"),
    })?;
    Ok(file.sandbox)
}

/// Load named network policies from `$XDG_CONFIG_HOME/sodagun/network-policies.toml`.
///
/// Returns `(map, None)` if the file does not exist, `(map, Some(path))` if it does.
/// Returns `CONFIG_INVALID` if the file exists but is malformed.
pub fn load_network_policies()
-> Result<(HashMap<String, NamedPolicy>, Option<PathBuf>), SodagunError> {
    let Some(path) = config_path("network-policies.toml") else {
        return Ok((HashMap::new(), None));
    };
    let (map, exists) = load_network_policies_from_path(&path)?;
    Ok((map, exists.then_some(path)))
}

pub(crate) fn load_network_policies_from_path(
    path: &Path,
) -> Result<(HashMap<String, NamedPolicy>, bool), SodagunError> {
    if !path.exists() {
        return Ok((HashMap::new(), false));
    }
    let contents = std::fs::read_to_string(path).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("failed to read network policies file: {e}"),
    })?;
    let policies: HashMap<String, NamedPolicy> =
        toml::from_str(&contents).map_err(|e| SodagunError {
            code: "CONFIG_INVALID",
            message: format!("invalid network policies TOML: {e}"),
        })?;
    // Reserved names are always built-in; redefining them would shadow the built-ins silently.
    for name in policies.keys() {
        if RESERVED_POLICY_NAMES.contains(&name.as_str()) {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: format!(
                    "network policy '{name}' is a reserved built-in name and cannot be redefined"
                ),
            });
        }
    }
    Ok((policies, true))
}

/// Merge user and project sandbox configs into a resolved [`SandboxConfig`].
///
/// Merge semantics:
/// - `volumes`: user first, then project appended
/// - `env` / `secrets`: union; project wins on conflict
/// - Scalars (`working_dir`, `memory_mb`, `cpus`): project > user > built-in default
/// - `network.policy` / `default_egress` / `default_ingress`: project > user
/// - `network.rules`: user inline first, then project inline
///
/// Validates that no key appears in both `env` and `secrets` after merging.
/// `env` values may be plain strings or dynamic sources (`value_from_env`, `value_from_cmd`);
/// dynamic sources are resolved at launch time, not here.
pub fn merge_sandbox_configs(
    user: Option<RawSandboxConfig>,
    project: RawSandboxConfig,
) -> Result<SandboxConfig, SodagunError> {
    let RawSandboxConfig {
        working_dir: user_wd,
        memory_mb: user_mem,
        cpus: user_cpus,
        volumes: user_vols,
        network: user_net,
        env: user_env,
        secrets: user_secrets,
    } = user.unwrap_or_default();

    let RawSandboxConfig {
        working_dir: proj_wd,
        memory_mb: proj_mem,
        cpus: proj_cpus,
        volumes: proj_vols,
        network: proj_net,
        env: proj_env,
        secrets: proj_secrets,
    } = project;

    // Volumes: user first, project appended
    let mut volumes = user_vols;
    volumes.extend(proj_vols);

    // Env and secrets: user base, project overwrites
    let mut env = user_env;
    env.extend(proj_env);

    let mut secrets = user_secrets;
    secrets.extend(proj_secrets);

    // Scalars: project > user > built-in default
    let working_dir = proj_wd.or(user_wd).unwrap_or_else(default_working_dir);
    let memory_mb = proj_mem.or(user_mem).unwrap_or_else(default_memory_mb);
    let cpus = proj_cpus.or(user_cpus).unwrap_or_else(default_cpus);

    // Network: project > user for scalars; rules concatenated
    let policy = proj_net.policy.or(user_net.policy);
    let default_egress = proj_net.default_egress.or(user_net.default_egress);
    let default_ingress = proj_net.default_ingress.or(user_net.default_ingress);
    let mut rules = user_net.rules;
    rules.extend(proj_net.rules);

    // Env/secret conflict check on merged result
    for key in secrets.keys() {
        if env.contains_key(key.as_str()) {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: format!("'{key}' appears in both [sandbox.env] and [sandbox.secrets]"),
            });
        }
    }

    Ok(SandboxConfig {
        working_dir,
        memory_mb,
        cpus,
        volumes,
        network: NetworkConfig {
            policy,
            default_egress,
            default_ingress,
            rules,
        },
        env,
        secrets,
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

// ── Private helpers ───────────────────────────────────────────────────────────

/// Returns `$XDG_CONFIG_HOME/sodagun/<filename>` or `$HOME/.config/sodagun/<filename>`.
/// Returns `None` when neither env var is set — that means no user config directory is
/// available, which callers treat as "no user config" (not an error).
fn config_path(filename: &str) -> Option<PathBuf> {
    xdg_config_path(&format!("sodagun/{filename}"))
}

/// Returns `$XDG_CONFIG_HOME/<rel>` or `$HOME/.config/<rel>`, or `None` if
/// neither env var is set.
fn xdg_config_path(rel: &str) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        Some(PathBuf::from(dir).join(rel))
    } else if let Ok(home) = std::env::var("HOME") {
        Some(PathBuf::from(home).join(".config").join(rel))
    } else {
        None
    }
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

// ── Tests ─────────────────────────────────────────────────────────────────────

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

    /// After the network redesign, scalars in `RawSandboxConfig` are `None` when absent
    /// (they get defaults only after `merge_sandbox_configs`).
    #[test]
    fn valid_sandbox_defaults() {
        let f = write_config("[image]\nbase_image = \"debian\"\n");
        let (_, raw) = load_config(f.path()).unwrap();
        assert!(raw.working_dir.is_none());
        assert!(raw.memory_mb.is_none());
        assert!(raw.cpus.is_none());
        assert!(raw.network.policy.is_none());
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
policy = "public-only"

[sandbox.env]
TERM = "xterm-256color"

[sandbox.secrets.ANTHROPIC_API_KEY]
value_from_env = "ANTHROPIC_API_KEY"
allowed_hosts = ["api.anthropic.com"]
"#,
        );
        let (_, raw) = load_config(f.path()).unwrap();
        assert_eq!(raw.memory_mb, Some(1024));
        assert_eq!(raw.cpus, Some(2));
        assert_eq!(raw.network.policy.as_deref(), Some("public-only"));
        assert_eq!(raw.volumes, ["~/.config/claude:/root/.config/claude:ro"]);
        assert!(matches!(
            raw.env.get("TERM"),
            Some(EnvValue::Literal(s)) if s == "xterm-256color"
        ));
        let secret = raw.secrets.get("ANTHROPIC_API_KEY").unwrap();
        assert_eq!(secret.value_from_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    /// The old `mode` field must be rejected because `NetworkConfig` uses `deny_unknown_fields`.
    #[test]
    fn error_mode_field_rejected() {
        let f =
            write_config("[image]\nbase_image = \"debian\"\n[sandbox.network]\nmode = \"none\"\n");
        let err = load_config(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn error_env_secret_conflict_still_detected_via_merge() {
        // Conflict is now detected in merge_sandbox_configs, not load_config.
        // This test verifies merge_sandbox_configs catches it.
        let mut raw = RawSandboxConfig::default();
        raw.env
            .insert("MY_KEY".to_string(), EnvValue::Literal("val".to_string()));
        raw.secrets.insert(
            "MY_KEY".to_string(),
            SecretConfig {
                value_from_env: Some("MY_KEY".to_string()),
                value: None,
                value_from_cmd: None,
                allowed_hosts: vec![],
            },
        );
        let err = merge_sandbox_configs(None, raw).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    /// `value_from_cmd` parses correctly from TOML.
    #[test]
    fn value_from_cmd_parses() {
        let f = write_config(
            r#"
[image]
base_image = "alpine"

[sandbox.secrets.MY_SECRET]
value_from_cmd = "security find-generic-password -w -s my-service"
allowed_hosts = []
"#,
        );
        let (_, raw) = load_config(f.path()).unwrap();
        let secret = raw.secrets.get("MY_SECRET").unwrap();
        assert_eq!(
            secret.value_from_cmd.as_deref(),
            Some("security find-generic-password -w -s my-service")
        );
        assert!(secret.value.is_none());
        assert!(secret.value_from_env.is_none());
    }

    /// `value_from_cmd` and `value` can coexist at parse time
    /// (conflict is detected at launch in `start_async`).
    #[test]
    fn value_from_cmd_with_value_parses() {
        let f = write_config(
            r#"
[image]
base_image = "alpine"

[sandbox.secrets.MY_SECRET]
value = "literal"
value_from_cmd = "echo secret"
allowed_hosts = []
"#,
        );
        let (_, raw) = load_config(f.path()).unwrap();
        let secret = raw.secrets.get("MY_SECRET").unwrap();
        assert_eq!(secret.value.as_deref(), Some("literal"));
        assert_eq!(secret.value_from_cmd.as_deref(), Some("echo secret"));
    }

    /// Dynamic `EnvValue` entries in `[sandbox.env]` parse correctly.
    #[test]
    fn env_value_dynamic_parses() {
        let f = write_config(
            r#"
[image]
base_image = "alpine"

[sandbox.env]
TERM = "xterm-256color"

[sandbox.env.MY_TOKEN]
value_from_cmd = "echo secret"
"#,
        );
        let (_, raw) = load_config(f.path()).unwrap();
        assert!(matches!(raw.env.get("TERM"), Some(EnvValue::Literal(s)) if s == "xterm-256color"));
        let dynamic = raw.env.get("MY_TOKEN").unwrap();
        assert!(
            matches!(dynamic, EnvValue::Dynamic(src) if src.value_from_cmd.as_deref() == Some("echo secret"))
        );
    }

    // ── load_network_policies tests ───────────────────────────────────────────

    #[test]
    fn load_network_policies_missing_file_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("network-policies.toml");
        let (map, exists) = load_network_policies_from_path(&path).unwrap();
        assert!(!exists);
        assert!(map.is_empty());
    }

    #[test]
    fn load_network_policies_valid_toml() {
        let f = write_config(
            r#"
[my-policy]
default_egress = "deny"
default_ingress = "allow"

[[my-policy.rules]]
direction = "egress"
action = "allow"
destination = "api.example.com"
"#,
        );
        let (map, exists) = load_network_policies_from_path(f.path()).unwrap();
        assert!(exists);
        let policy = map.get("my-policy").unwrap();
        assert_eq!(policy.default_egress, Some(ConfigAction::Deny));
        assert_eq!(policy.default_ingress, Some(ConfigAction::Allow));
        assert_eq!(policy.rules.len(), 1);
        assert_eq!(policy.rules[0].destination, "api.example.com");
    }

    #[test]
    fn load_network_policies_malformed_toml() {
        let f = write_config("not valid toml @@@@");
        let err = load_network_policies_from_path(f.path()).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn load_network_policies_reserved_name_rejected() {
        for reserved in RESERVED_POLICY_NAMES {
            let toml = format!("[{reserved}]\ndefault_egress = \"deny\"\n");
            let f = write_config(&toml);
            let err = load_network_policies_from_path(f.path()).unwrap_err();
            assert_eq!(
                err.code, "CONFIG_INVALID",
                "expected error for '{reserved}'"
            );
            assert!(
                err.message.contains(reserved),
                "error should mention the reserved name '{reserved}'; got: {}",
                err.message
            );
        }
    }

    // ── merge_sandbox_configs tests ───────────────────────────────────────────

    #[test]
    fn merge_volumes_concatenated_user_first() {
        let user = RawSandboxConfig {
            volumes: vec!["~/.config/claude:/root/.config/claude:ro".to_string()],
            ..Default::default()
        };
        let project = RawSandboxConfig {
            volumes: vec!["/data:/data".to_string()],
            ..Default::default()
        };

        let merged = merge_sandbox_configs(Some(user), project).unwrap();
        assert_eq!(
            merged.volumes,
            ["~/.config/claude:/root/.config/claude:ro", "/data:/data"]
        );
    }

    #[test]
    fn merge_env_project_wins_on_conflict() {
        let mut user = RawSandboxConfig::default();
        user.env
            .insert("TERM".to_string(), EnvValue::Literal("dumb".to_string()));
        user.env.insert(
            "USER_ONLY".to_string(),
            EnvValue::Literal("yes".to_string()),
        );

        let mut project = RawSandboxConfig::default();
        project.env.insert(
            "TERM".to_string(),
            EnvValue::Literal("xterm-256color".to_string()),
        );

        let merged = merge_sandbox_configs(Some(user), project).unwrap();
        assert!(
            matches!(merged.env.get("TERM"), Some(EnvValue::Literal(s)) if s == "xterm-256color")
        );
        assert!(matches!(merged.env.get("USER_ONLY"), Some(EnvValue::Literal(s)) if s == "yes"));
    }

    #[test]
    fn merge_env_secret_conflict_returns_error() {
        let mut project = RawSandboxConfig::default();
        project
            .env
            .insert("MY_KEY".to_string(), EnvValue::Literal("val".to_string()));
        project.secrets.insert(
            "MY_KEY".to_string(),
            SecretConfig {
                value_from_env: Some("MY_KEY".to_string()),
                value: None,
                value_from_cmd: None,
                allowed_hosts: vec![],
            },
        );
        let err = merge_sandbox_configs(None, project).unwrap_err();
        assert_eq!(err.code, "CONFIG_INVALID");
    }

    #[test]
    fn merge_scalars_project_wins_over_user() {
        let user = RawSandboxConfig {
            working_dir: Some("/user".to_string()),
            memory_mb: Some(256),
            cpus: Some(1),
            ..Default::default()
        };
        let project = RawSandboxConfig {
            working_dir: Some("/project".to_string()),
            memory_mb: Some(1024),
            ..Default::default()
        };

        let merged = merge_sandbox_configs(Some(user), project).unwrap();
        assert_eq!(merged.working_dir, "/project");
        assert_eq!(merged.memory_mb, 1024);
        // cpus: only user has it, project doesn't → user wins
        assert_eq!(merged.cpus, 1);
    }

    #[test]
    fn merge_scalars_defaults_when_neither_set() {
        let merged = merge_sandbox_configs(None, RawSandboxConfig::default()).unwrap();
        assert_eq!(merged.working_dir, "/workspace");
        assert_eq!(merged.memory_mb, 512);
        assert_eq!(merged.cpus, 1);
    }

    #[test]
    fn merge_network_policy_project_wins() {
        let mut user = RawSandboxConfig::default();
        user.network.policy = Some("allow-all".to_string());

        let mut project = RawSandboxConfig::default();
        project.network.policy = Some("none".to_string());

        let merged = merge_sandbox_configs(Some(user), project).unwrap();
        assert_eq!(merged.network.policy.as_deref(), Some("none"));
    }

    #[test]
    fn merge_network_rules_concatenated_user_first() {
        let mut user = RawSandboxConfig::default();
        user.network.rules = vec![NetworkRule {
            direction: ConfigDirection::Egress,
            action: ConfigAction::Allow,
            destination: "user-api.example.com".to_string(),
            protocol: None,
            ports: vec![],
        }];

        let mut project = RawSandboxConfig::default();
        project.network.rules = vec![NetworkRule {
            direction: ConfigDirection::Egress,
            action: ConfigAction::Allow,
            destination: "project-api.example.com".to_string(),
            protocol: None,
            ports: vec![],
        }];

        let merged = merge_sandbox_configs(Some(user), project).unwrap();
        assert_eq!(merged.network.rules.len(), 2);
        assert_eq!(merged.network.rules[0].destination, "user-api.example.com");
        assert_eq!(
            merged.network.rules[1].destination,
            "project-api.example.com"
        );
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
