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
    assert!(img.dockerfile.is_none());
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
fn valid_image_dockerfile() {
    let tmp = TempDir::new().unwrap();
    let df = tmp.path().join("Dockerfile");
    std::fs::write(&df, "FROM alpine\n").unwrap();
    let config_content =
        "[image]\ndockerfile = \"Dockerfile\"\nnamespace_repository = \"org/repo\"\n".to_string();
    let mut cfg = NamedTempFile::new_in(tmp.path()).unwrap();
    cfg.write_all(config_content.as_bytes()).unwrap();
    let (img, _) = load_config(cfg.path()).unwrap();
    assert!(img.dockerfile.is_some());
    assert!(img.dockerfile.as_ref().unwrap().ends_with("Dockerfile"));
    assert_eq!(img.namespace_repository.as_deref(), Some("org/repo"));
    assert!(img.base_image.is_none());
    assert!(img.base_snapshot.is_none());
}

#[test]
fn valid_image_dockerfile_with_version() {
    let tmp = TempDir::new().unwrap();
    let df = tmp.path().join("Dockerfile");
    std::fs::write(&df, "FROM alpine\n").unwrap();
    let config_content =
        "dockerfile = \"Dockerfile\"\nnamespace_repository = \"org/repo\"\nversion = \"2\"\n";
    let toml = format!("[image]\n{config_content}");
    let mut cfg = NamedTempFile::new_in(tmp.path()).unwrap();
    cfg.write_all(toml.as_bytes()).unwrap();
    let (img, _) = load_config(cfg.path()).unwrap();
    assert_eq!(img.version.as_deref(), Some("2"));
}

#[test]
fn error_image_neither_base_nor_dockerfile() {
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
fn error_dockerfile_and_base_image_conflict() {
    let tmp = TempDir::new().unwrap();
    let df = tmp.path().join("Dockerfile");
    std::fs::write(&df, "FROM alpine\n").unwrap();
    let toml = "dockerfile = \"Dockerfile\"\nbase_image = \"alpine\"\nnamespace_repository = \"org/repo\"\n";
    let mut cfg = NamedTempFile::new_in(tmp.path()).unwrap();
    cfg.write_all(format!("[image]\n{toml}").as_bytes())
        .unwrap();
    let err = load_config(cfg.path()).unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("mutually exclusive"));
}

#[test]
fn error_dockerfile_and_base_snapshot_conflict() {
    let tmp = TempDir::new().unwrap();
    let df = tmp.path().join("Dockerfile");
    std::fs::write(&df, "FROM alpine\n").unwrap();
    let toml = "dockerfile = \"Dockerfile\"\nbase_snapshot = \"my-snap\"\nnamespace_repository = \"org/repo\"\n";
    let mut cfg = NamedTempFile::new_in(tmp.path()).unwrap();
    cfg.write_all(format!("[image]\n{toml}").as_bytes())
        .unwrap();
    let err = load_config(cfg.path()).unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("mutually exclusive"));
}

#[test]
fn dockerfile_without_namespace_repository_loads_ok() {
    // namespace_repository is NOT required at parse time: the user config can supply it.
    // The error is deferred to dockerfile_image_tag() at tag-compute time.
    let tmp = TempDir::new().unwrap();
    let df = tmp.path().join("Dockerfile");
    std::fs::write(&df, "FROM alpine\n").unwrap();
    let mut cfg = NamedTempFile::new_in(tmp.path()).unwrap();
    cfg.write_all(b"[image]\ndockerfile = \"Dockerfile\"\n")
        .unwrap();
    let (img, _) = load_config(cfg.path()).unwrap();
    assert!(img.namespace_repository.is_none());
}

#[test]
fn error_dockerfile_path_not_found() {
    let f = write_config("[image]\ndockerfile = \"./nonexistent.Dockerfile\"\n");
    let err = load_config(f.path()).unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("does not exist"), "{}", err.message);
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
    let f = write_config("[image]\nbase_image = \"debian\"\n[sandbox.network]\nmode = \"none\"\n");
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
    assert!(matches!(merged.env.get("TERM"), Some(EnvValue::Literal(s)) if s == "xterm-256color"));
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

// ── merge_registry_configs tests ──────────────────────────────────────────

#[test]
fn merge_registry_project_wins() {
    let user = RegistryConfig {
        host: Some("user.registry.io".to_string()),
        insecure: Some(false),
    };
    let project = RegistryConfig {
        host: Some("project.registry.io".to_string()),
        insecure: None,
    };
    let merged = merge_registry_configs(user, project);
    assert_eq!(merged.host.as_deref(), Some("project.registry.io"));
    // insecure: only user has it → user wins
    assert_eq!(merged.insecure, Some(false));
}

#[test]
fn merge_registry_user_fills_absent_project_fields() {
    let user = RegistryConfig {
        host: Some("user.registry.io".to_string()),
        insecure: Some(true),
    };
    let project = RegistryConfig::default();
    let merged = merge_registry_configs(user, project);
    assert_eq!(merged.host.as_deref(), Some("user.registry.io"));
    assert_eq!(merged.insecure, Some(true));
}

// ── load_user_image_config / merge_user_image_config tests ───────────────

#[test]
fn load_user_image_config_parses_fields() {
    let f = write_config(
        r#"
[image]
namespace_repository = "myorg/myrepo"
version = "3"
"#,
    );
    let cfg = load_user_image_config_from_path(f.path()).unwrap();
    assert_eq!(cfg.namespace_repository.as_deref(), Some("myorg/myrepo"));
    assert_eq!(cfg.version.as_deref(), Some("3"));
}

#[test]
fn load_user_image_config_missing_file_returns_default() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("sodagun.toml");
    let cfg = load_user_image_config_from_path(&path).unwrap();
    assert!(cfg.namespace_repository.is_none());
    assert!(cfg.version.is_none());
}

#[test]
fn merge_user_image_config_project_wins_on_conflict() {
    let user = UserImageConfig {
        namespace_repository: Some("user/repo".to_string()),
        version: Some("5".to_string()),
    };
    let project = ImageConfig {
        base_image: None,
        base_snapshot: None,
        dockerfile: None,
        namespace_repository: Some("proj/repo".to_string()),
        version: Some("2".to_string()),
    };
    let merged = merge_user_image_config(user, project);
    assert_eq!(merged.namespace_repository.as_deref(), Some("proj/repo"));
    assert_eq!(merged.version.as_deref(), Some("2"));
}

#[test]
fn merge_user_image_config_user_fills_absent_project_fields() {
    let user = UserImageConfig {
        namespace_repository: Some("user/repo".to_string()),
        version: Some("5".to_string()),
    };
    let project = ImageConfig {
        base_image: None,
        base_snapshot: None,
        dockerfile: None,
        namespace_repository: None,
        version: None,
    };
    let merged = merge_user_image_config(user, project);
    assert_eq!(merged.namespace_repository.as_deref(), Some("user/repo"));
    assert_eq!(merged.version.as_deref(), Some("5"));
}

// ── dockerfile_image_tag tests ────────────────────────────────────────────

#[test]
fn dockerfile_image_tag_deterministic() {
    let img = ImageConfig {
        base_image: None,
        base_snapshot: None,
        dockerfile: None,
        namespace_repository: Some("org/repo".to_string()),
        version: None,
    };
    let reg = RegistryConfig {
        host: Some("registry.example.com".to_string()),
        insecure: None,
    };
    let dockerfile = b"FROM alpine\nRUN apk add git\n";
    let a = dockerfile_image_tag(&img, &reg, dockerfile).unwrap();
    let b = dockerfile_image_tag(&img, &reg, dockerfile).unwrap();
    assert_eq!(a, b);
}

#[test]
fn dockerfile_image_tag_changes_with_content() {
    let img = ImageConfig {
        base_image: None,
        base_snapshot: None,
        dockerfile: None,
        namespace_repository: Some("org/repo".to_string()),
        version: None,
    };
    let reg = RegistryConfig {
        host: Some("registry.example.com".to_string()),
        insecure: None,
    };
    let a = dockerfile_image_tag(&img, &reg, b"FROM alpine\n").unwrap();
    let b = dockerfile_image_tag(&img, &reg, b"FROM debian\n").unwrap();
    assert_ne!(a, b);
}

#[test]
fn dockerfile_image_tag_changes_with_version() {
    let img_v1 = ImageConfig {
        base_image: None,
        base_snapshot: None,
        dockerfile: None,
        namespace_repository: Some("org/repo".to_string()),
        version: None, // defaults to "1"
    };
    let img_v2 = ImageConfig {
        base_image: None,
        base_snapshot: None,
        dockerfile: None,
        namespace_repository: Some("org/repo".to_string()),
        version: Some("2".to_string()),
    };
    let reg = RegistryConfig {
        host: Some("registry.example.com".to_string()),
        insecure: None,
    };
    let dockerfile = b"FROM alpine\n";
    let a = dockerfile_image_tag(&img_v1, &reg, dockerfile).unwrap();
    let b = dockerfile_image_tag(&img_v2, &reg, dockerfile).unwrap();
    assert_ne!(a, b);
}

#[test]
fn dockerfile_image_tag_correct_format() {
    let img = ImageConfig {
        base_image: None,
        base_snapshot: None,
        dockerfile: None,
        namespace_repository: Some("org/repo".to_string()),
        version: None,
    };
    let reg = RegistryConfig {
        host: Some("registry.example.com".to_string()),
        insecure: None,
    };
    let tag = dockerfile_image_tag(&img, &reg, b"FROM alpine\n").unwrap();
    // format: <host>/<namespace_repository>:v<12-char-sha>
    assert!(tag.starts_with("registry.example.com/org/repo:"));
    let sha = tag.rsplit_once(':').unwrap().1;
    assert_eq!(sha.len(), 13);
    assert!(sha.starts_with('v'));
}

#[test]
fn dockerfile_image_tag_missing_host_error() {
    let img = ImageConfig {
        base_image: None,
        base_snapshot: None,
        dockerfile: None,
        namespace_repository: Some("org/repo".to_string()),
        version: None,
    };
    let reg = RegistryConfig::default(); // no host
    let err = dockerfile_image_tag(&img, &reg, b"FROM alpine\n").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("registry.host"));
}

#[test]
fn dockerfile_image_tag_missing_namespace_error() {
    let img = ImageConfig {
        base_image: None,
        base_snapshot: None,
        dockerfile: None,
        namespace_repository: None, // missing
        version: None,
    };
    let reg = RegistryConfig {
        host: Some("registry.example.com".to_string()),
        insecure: None,
    };
    let err = dockerfile_image_tag(&img, &reg, b"FROM alpine\n").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("namespace_repository"));
}

// ── parse_volume tests ────────────────────────────────────────────────────

#[test]
fn parse_volume_basic() {
    let (host, guest, flags) = parse_volume("/host/path:/guest/path").unwrap();
    assert_eq!(host, PathBuf::from("/host/path"));
    assert_eq!(guest, "/guest/path");
    assert_eq!(flags, MountFlags::default());
}

#[test]
fn parse_volume_readonly() {
    let (_, _, flags) = parse_volume("/host:/guest:ro").unwrap();
    assert!(flags.readonly);
    assert!(!flags.noexec);
}

#[test]
fn parse_volume_noexec() {
    let (_, _, flags) = parse_volume("/host:/guest:noexec").unwrap();
    assert!(!flags.readonly);
    assert!(flags.noexec);
}

#[test]
fn parse_volume_ro_and_noexec() {
    let (_, _, flags) = parse_volume("/host:/guest:ro,noexec").unwrap();
    assert!(flags.readonly);
    assert!(flags.noexec);
}

#[test]
fn parse_volume_explicit_rw() {
    // rw is a no-op — read-write is already the default
    let (_, _, flags) = parse_volume("/host:/guest:rw").unwrap();
    assert_eq!(flags, MountFlags::default());
}

#[test]
fn parse_volume_unknown_option_error() {
    let err = parse_volume("/host:/guest:nosuid").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("nosuid"), "{}", err.message);
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
