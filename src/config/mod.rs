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

/// Mount options parsed from the options segment of a Docker-style volume string.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MountFlags {
    /// Whether the mount is read-only (`ro`).
    pub readonly: bool,
    /// Whether direct execution from the mount is disabled (`noexec`).
    pub noexec: bool,
}

/// Parse a Docker-style volume string: `"host:guest"` or `"host:guest:OPTIONS"`,
/// where OPTIONS is a comma-separated list of `ro`, `rw`, `noexec`.
///
/// A leading `~` in the host path is expanded to `$HOME` here, at launch time,
/// rather than at config-parse time — so the parsed [`SandboxConfig`] never
/// carries an environment-dependent absolute path.
pub fn parse_volume(s: &str) -> Result<(PathBuf, String, MountFlags), SodagunError> {
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    if parts.len() < 2 {
        return Err(SodagunError {
            code: "CONFIG_INVALID",
            message: format!("volume '{s}' must be 'host:guest' or 'host:guest:OPTIONS'"),
        });
    }
    let host_raw = parts[0];
    let guest = parts[1].to_string();
    let flags = parse_mount_flags(parts.get(2).copied().unwrap_or(""), s)?;

    let host = if let Some(rest) = host_raw.strip_prefix('~') {
        let home = std::env::var("HOME").map_err(|_| SodagunError {
            code: "CONFIG_INVALID",
            message: "cannot expand '~' in volume: $HOME is not set".to_string(),
        })?;
        PathBuf::from(format!("{home}{rest}"))
    } else {
        PathBuf::from(host_raw)
    };

    Ok((host, guest, flags))
}

/// Parse comma-separated mount options (`ro`, `rw`, `noexec`) into [`MountFlags`].
fn parse_mount_flags(opts: &str, vol: &str) -> Result<MountFlags, SodagunError> {
    let mut flags = MountFlags::default();
    for opt in opts.split(',').filter(|o| !o.is_empty()) {
        match opt {
            "ro" => flags.readonly = true,
            "rw" => {} // explicit rw is a no-op (read-write is the default)
            "noexec" => flags.noexec = true,
            _ => {
                return Err(SodagunError {
                    code: "CONFIG_INVALID",
                    message: format!("unknown mount option '{opt}' in volume '{vol}'"),
                });
            }
        }
    }
    Ok(flags)
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

#[cfg(test)]
mod tests;
