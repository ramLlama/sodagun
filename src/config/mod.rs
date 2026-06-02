use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::SodagunError;

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

// ── Registry config ───────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct RawRegistryConfig {
    host: Option<String>,
    insecure: Option<bool>,
}

/// Resolved OCI registry configuration (from `[registry]` in sodagun.toml or user config).
#[derive(Debug, Default)]
pub struct RegistryConfig {
    pub host: Option<String>,
    pub insecure: Option<bool>,
}

// ── User-level image overrides ────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct RawUserImageConfig {
    namespace_repository: Option<String>,
    version: Option<String>,
}

/// User-level image configuration overrides from `~/.config/sodagun/sodagun.toml`.
#[derive(Debug, Default)]
pub struct UserImageConfig {
    pub namespace_repository: Option<String>,
    pub version: Option<String>,
}

// ── Image config ──────────────────────────────────────────────────────────────

/// Resolved image configuration from the `[image]` TOML table.
#[derive(Debug)]
pub struct ImageConfig {
    pub base_image: Option<String>,
    pub base_snapshot: Option<String>,
    /// Resolved absolute path to the Dockerfile; `None` if no dockerfile configured.
    pub dockerfile: Option<PathBuf>,
    pub namespace_repository: Option<String>,
    /// Version tag component; `None` is treated as `"1"` by [`dockerfile_image_tag`].
    pub version: Option<String>,
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
/// alpine:latest with no setup.
pub fn default_image_config() -> ImageConfig {
    ImageConfig {
        base_image: Some("alpine:latest".to_string()),
        base_snapshot: None,
        dockerfile: None,
        namespace_repository: None,
        version: None,
    }
}

// ── TOML deserialization ──────────────────────────────────────────────────────

/// Raw deserialization struct for `[image]` — before validation / file resolution.
#[derive(Deserialize)]
struct RawImageConfig {
    base_image: Option<String>,
    base_snapshot: Option<String>,
    dockerfile: Option<String>,
    namespace_repository: Option<String>,
    version: Option<String>,
}

/// Top-level wrapper matching the TOML file structure.
#[derive(Deserialize)]
struct ConfigFile {
    image: Option<RawImageConfig>,
    sandbox: Option<RawSandboxConfig>,
    registry: Option<RawRegistryConfig>,
}

/// Minimal wrapper for the user-level config file.
#[derive(Deserialize)]
struct UserConfigFile {
    sandbox: Option<RawSandboxConfig>,
    image: Option<RawUserImageConfig>,
    registry: Option<RawRegistryConfig>,
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

/// Load the `[registry]` section from a project config file at `path`.
///
/// Returns `RegistryConfig::default()` if the section is absent.
/// Returns `CONFIG_INVALID` if the file exists but contains invalid TOML.
pub fn load_registry_config(path: &Path) -> Result<RegistryConfig, SodagunError> {
    if !path.exists() {
        return Ok(RegistryConfig::default());
    }
    let contents = std::fs::read_to_string(path).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("failed to read config: {e}"),
    })?;
    let file: ConfigFile = toml::from_str(&contents).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("invalid TOML: {e}"),
    })?;
    Ok(raw_registry_to_config(file.registry.unwrap_or_default()))
}

/// Load the `[registry]` section from the user-level config file
/// (`$XDG_CONFIG_HOME/sodagun/sodagun.toml`).
///
/// Returns `RegistryConfig::default()` if the file or section is absent.
pub fn load_user_registry_config() -> Result<RegistryConfig, SodagunError> {
    let Some(path) = config_path("sodagun.toml") else {
        return Ok(RegistryConfig::default());
    };
    load_user_registry_config_from_path(&path)
}

pub(crate) fn load_user_registry_config_from_path(
    path: &Path,
) -> Result<RegistryConfig, SodagunError> {
    if !path.exists() {
        return Ok(RegistryConfig::default());
    }
    let contents = std::fs::read_to_string(path).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("failed to read user config: {e}"),
    })?;
    let file: UserConfigFile = toml::from_str(&contents).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("invalid TOML in user config: {e}"),
    })?;
    Ok(raw_registry_to_config(file.registry.unwrap_or_default()))
}

/// Merge user and project registry configs; project wins on each field.
pub fn merge_registry_configs(user: RegistryConfig, project: RegistryConfig) -> RegistryConfig {
    RegistryConfig {
        host: project.host.or(user.host),
        insecure: project.insecure.or(user.insecure),
    }
}

/// Load `[image].namespace_repository` and `[image].version` from the user config file.
///
/// Returns `UserImageConfig::default()` if the file or section is absent.
pub fn load_user_image_config() -> Result<UserImageConfig, SodagunError> {
    let Some(path) = config_path("sodagun.toml") else {
        return Ok(UserImageConfig::default());
    };
    load_user_image_config_from_path(&path)
}

pub(crate) fn load_user_image_config_from_path(
    path: &Path,
) -> Result<UserImageConfig, SodagunError> {
    if !path.exists() {
        return Ok(UserImageConfig::default());
    }
    let contents = std::fs::read_to_string(path).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("failed to read user config: {e}"),
    })?;
    let file: UserConfigFile = toml::from_str(&contents).map_err(|e| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("invalid TOML in user config: {e}"),
    })?;
    let raw = file.image.unwrap_or_default();
    Ok(UserImageConfig {
        namespace_repository: raw.namespace_repository,
        version: raw.version,
    })
}

/// Apply user image overrides to a project `ImageConfig`.
///
/// Project wins on each field where both are `Some`; user values fill in where
/// the project field is `None`.
pub fn merge_user_image_config(user: UserImageConfig, mut project: ImageConfig) -> ImageConfig {
    project.namespace_repository = project.namespace_repository.or(user.namespace_repository);
    project.version = project.version.or(user.version);
    project
}

/// Compute the full OCI image tag for a Dockerfile-based image.
///
/// Tag format: `<host>/<namespace_repository>:<sha>`
/// where `sha` = first 12 base64url chars of SHA-256(dockerfile_bytes ‖ version_bytes).
///
/// Returns `CONFIG_INVALID` if `registry.host` or `image_config.namespace_repository` is unset.
pub fn dockerfile_image_tag(
    image_config: &ImageConfig,
    registry: &RegistryConfig,
    dockerfile_bytes: &[u8],
) -> Result<String, SodagunError> {
    let ns_repo = image_config
        .namespace_repository
        .as_deref()
        .ok_or_else(|| SodagunError {
            code: "CONFIG_INVALID",
            message: "image.namespace_repository is required when using a dockerfile".to_string(),
        })?;
    let host = registry.host.as_deref().ok_or_else(|| SodagunError {
        code: "CONFIG_INVALID",
        message: "registry.host is required when using a dockerfile".to_string(),
    })?;
    let version_bytes = image_config.version.as_deref().unwrap_or("1").as_bytes();

    let mut hasher = Sha256::new();
    hasher.update(dockerfile_bytes);
    hasher.update(version_bytes);
    let hash = hasher.finalize();
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&hash[..]);
    // SHA-256 → 32 bytes → base64url (no pad) → 43 chars; [..12] is always in range.
    let sha = &b64[..12];

    Ok(format!("{host}/{ns_repo}:{sha}"))
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

fn raw_registry_to_config(raw: RawRegistryConfig) -> RegistryConfig {
    RegistryConfig {
        host: raw.host,
        insecure: raw.insecure,
    }
}

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
    let has_base_image = raw.base_image.is_some();
    let has_base_snapshot = raw.base_snapshot.is_some();
    let has_dockerfile = raw.dockerfile.is_some();

    // dockerfile is mutually exclusive with base_image and base_snapshot
    if has_dockerfile && has_base_image {
        return Err(SodagunError {
            code: "CONFIG_INVALID",
            message: "'dockerfile' and 'base_image' are mutually exclusive in [image]".to_string(),
        });
    }
    if has_dockerfile && has_base_snapshot {
        return Err(SodagunError {
            code: "CONFIG_INVALID",
            message: "'dockerfile' and 'base_snapshot' are mutually exclusive in [image]"
                .to_string(),
        });
    }

    // Without dockerfile, exactly one of base_image / base_snapshot is required
    if !has_dockerfile {
        match (&raw.base_image, &raw.base_snapshot) {
            (None, None) => {
                return Err(SodagunError {
                    code: "CONFIG_INVALID",
                    message: "one of 'base_image', 'base_snapshot', or 'dockerfile' is required in [image]"
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
    }

    // Resolve dockerfile path
    let dockerfile = if let Some(ref df_path) = raw.dockerfile {
        let abs = config_path.parent().unwrap_or(Path::new(".")).join(df_path);
        if !abs.is_file() {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: format!(
                    "dockerfile '{}' does not exist or is not a file",
                    abs.display()
                ),
            });
        }
        Some(abs)
    } else {
        None
    };

    Ok(ImageConfig {
        base_image: raw.base_image,
        base_snapshot: raw.base_snapshot,
        dockerfile,
        namespace_repository: raw.namespace_repository,
        version: raw.version,
    })
}

#[cfg(test)]
mod tests;
