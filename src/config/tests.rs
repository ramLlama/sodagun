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
fn load_network_policies_missing_dir_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("network-policy.d");
    let (map, exists) = load_network_policies_from_dir(&dir).unwrap();
    assert!(!exists);
    assert!(map.is_empty());
}

#[test]
fn load_network_policies_valid_dir() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("network-policy.d");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("my-policy.toml"),
        r#"
default_egress = "deny"
default_ingress = "allow"

[[rules]]
direction = "egress"
action = "allow"
destination = "api.example.com"
"#,
    )
    .unwrap();
    let (map, exists) = load_network_policies_from_dir(&dir).unwrap();
    assert!(exists);
    let policy = map.get("my-policy").unwrap();
    assert_eq!(policy.default_egress, Some(ConfigAction::Deny));
    assert_eq!(policy.default_ingress, Some(ConfigAction::Allow));
    assert_eq!(policy.rules.len(), 1);
    assert_eq!(policy.rules[0].destination, "api.example.com");
}

#[test]
fn load_network_policies_non_toml_files_ignored() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("network-policy.d");
    std::fs::create_dir(&dir).unwrap();
    // A README alongside the policy files must not cause an error.
    std::fs::write(dir.join("README.md"), "# policies").unwrap();
    std::fs::write(dir.join("my-policy.toml"), "default_egress = \"deny\"\n").unwrap();
    let (map, exists) = load_network_policies_from_dir(&dir).unwrap();
    assert!(exists);
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("my-policy"));
}

#[test]
fn load_network_policies_malformed_toml() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("network-policy.d");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("bad.toml"), "not valid toml @@@@").unwrap();
    let err = load_network_policies_from_dir(&dir).unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
}

#[test]
fn load_network_policies_reserved_name_rejected() {
    for reserved in RESERVED_POLICY_NAMES {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("network-policy.d");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{reserved}.toml")),
            "default_egress = \"deny\"\n",
        )
        .unwrap();
        let err = load_network_policies_from_dir(&dir).unwrap_err();
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

/// `git_access` parses from TOML and merges with project > user > default none.
#[test]
fn git_access_parses_and_merges() {
    let f = write_config("[image]\nbase_image = \"debian\"\n[sandbox]\ngit_access = \"data\"\n");
    let (_, raw) = load_config(f.path()).unwrap();
    assert_eq!(raw.git_access, Some(GitAccess::Data));

    // project wins over user
    let user = RawSandboxConfig {
        git_access: Some(GitAccess::Full),
        ..Default::default()
    };
    let merged = merge_sandbox_configs(Some(user), raw).unwrap();
    assert_eq!(merged.git_access, GitAccess::Data);

    // default is none
    let merged = merge_sandbox_configs(None, RawSandboxConfig::default()).unwrap();
    assert_eq!(merged.git_access, GitAccess::None);
}

/// Unknown `git_access` values are rejected.
#[test]
fn git_access_invalid_value_rejected() {
    let f =
        write_config("[image]\nbase_image = \"debian\"\n[sandbox]\ngit_access = \"sometimes\"\n");
    let err = load_config(f.path()).unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
}
