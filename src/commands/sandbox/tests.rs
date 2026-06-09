use std::collections::HashMap;

use crate::config::{ConfigAction, ConfigDirection, ConfigProtocol, NamedPolicy, NetworkRule};
use microsandbox::NetworkPolicy;
use microsandbox_network::policy::{Action, Direction, Protocol};

use super::build_guest_invocation;
use super::network::{apply_named_policy, apply_rule, parse_net_rule_value};
use super::validate_and_extract_env_kv;

// ── parse_net_rule_value / parse_net_rule_spec tests ─────────────────────────

/// Minimal spec `action@destination` (no proto, no port).
#[test]
fn net_rule_spec_minimal_allow() {
    let rules = parse_net_rule_value("allow@host").unwrap();
    assert_eq!(rules.len(), 1);
    let r = &rules[0];
    assert_eq!(r.action, ConfigAction::Allow);
    assert_eq!(r.destination, "host");
    assert_eq!(r.direction, ConfigDirection::Egress);
    assert!(r.protocol.is_none());
    assert!(r.ports.is_empty());
}

/// Deny action is parsed correctly.
#[test]
fn net_rule_spec_deny_action() {
    let rules = parse_net_rule_value("deny@public").unwrap();
    assert_eq!(rules[0].action, ConfigAction::Deny);
    assert_eq!(rules[0].destination, "public");
}

/// Full spec with protocol and port.
#[test]
fn net_rule_spec_with_proto_and_port() {
    let rules = parse_net_rule_value("allow@host:tcp:9999").unwrap();
    let r = &rules[0];
    assert_eq!(r.action, ConfigAction::Allow);
    assert_eq!(r.destination, "host");
    assert_eq!(r.protocol, Some(ConfigProtocol::Tcp));
    assert_eq!(r.ports, vec![9999u16]);
}

/// UDP protocol is parsed correctly.
#[test]
fn net_rule_spec_udp_protocol() {
    let rules = parse_net_rule_value("allow@host:udp:53").unwrap();
    assert_eq!(rules[0].protocol, Some(ConfigProtocol::Udp));
    assert_eq!(rules[0].ports, vec![53u16]);
}

/// Protocol without a port.
#[test]
fn net_rule_spec_proto_no_port() {
    let rules = parse_net_rule_value("allow@api.example.com:tcp").unwrap();
    let r = &rules[0];
    assert_eq!(r.protocol, Some(ConfigProtocol::Tcp));
    assert!(r.ports.is_empty());
}

/// CIDR destination.
#[test]
fn net_rule_spec_cidr_destination() {
    let rules = parse_net_rule_value("deny@10.0.0.0/8").unwrap();
    assert_eq!(rules[0].destination, "10.0.0.0/8");
}

/// Comma-separated specs in one --net-rule value produce multiple rules.
#[test]
fn net_rule_value_comma_separated() {
    let rules = parse_net_rule_value("allow@host:tcp:9999,deny@public").unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].destination, "host");
    assert_eq!(rules[0].ports, vec![9999u16]);
    assert_eq!(rules[1].destination, "public");
    assert_eq!(rules[1].action, ConfigAction::Deny);
}

/// Whitespace around comma-separated specs is trimmed.
#[test]
fn net_rule_value_trims_whitespace() {
    let rules = parse_net_rule_value("allow@host , deny@public").unwrap();
    assert_eq!(rules.len(), 2);
}

/// Missing `@` separator → CONFIG_INVALID.
#[test]
fn net_rule_spec_missing_at_sign() {
    let err = parse_net_rule_value("allowhost").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("allowhost"));
}

/// Unknown action → CONFIG_INVALID.
#[test]
fn net_rule_spec_invalid_action() {
    let err = parse_net_rule_value("permit@host").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("permit"));
}

/// Unknown protocol → CONFIG_INVALID.
#[test]
fn net_rule_spec_invalid_protocol() {
    let err = parse_net_rule_value("allow@host:icmp:9999").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("icmp"));
}

/// Non-numeric port → CONFIG_INVALID.
#[test]
fn net_rule_spec_invalid_port() {
    let err = parse_net_rule_value("allow@host:tcp:nope").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("nope"));
}

/// Port 0 is a valid u16 and accepted (the firewall rule is unusual but not invalid).
#[test]
fn net_rule_spec_port_zero_accepted() {
    let rules = parse_net_rule_value("allow@host:tcp:0").unwrap();
    assert_eq!(rules[0].ports, vec![0u16]);
}

// ── validate_and_extract_env_kv tests ────────────────────────────────────────

#[test]
fn validate_env_kv_simple_key() {
    let (k, v) = validate_and_extract_env_kv("FOO=bar").unwrap();
    assert_eq!(k, "FOO");
    assert_eq!(v, "bar");
}

#[test]
fn validate_env_kv_key_with_spaces() {
    assert!(validate_and_extract_env_kv("MY KEY=value").is_ok());
}

/// Missing `=` separator is now caught inside the function.
#[test]
fn validate_env_kv_missing_equals_rejected() {
    let err = validate_and_extract_env_kv("FOOBAR").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("KEY=VALUE"));
}

/// A newline in the key would break the `export KEY=VAL` shell construct.
#[test]
fn validate_env_kv_newline_in_key_rejected() {
    let err = validate_and_extract_env_kv("FOO\nBAR=value").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
}

/// NUL byte in key is rejected.
#[test]
fn validate_env_kv_nul_in_key_rejected() {
    let err = validate_and_extract_env_kv("FOO\0BAR=value").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
}

/// Trailing newline in value is rejected (control character).
#[test]
fn validate_env_kv_newline_in_value_rejected() {
    let err = validate_and_extract_env_kv("FOOBAR=baz\n").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
}

/// NUL byte in value is rejected.
#[test]
fn validate_env_kv_nul_in_value_rejected() {
    let err = validate_and_extract_env_kv("FOOBAR=baz\0qux").unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
}

// ── build_guest_invocation tests ──────────────────────────────────────────────

fn cmd(s: &str) -> Vec<String> {
    vec![s.to_string()]
}

fn cmd_args(s: &str, rest: &[&str]) -> Vec<String> {
    std::iter::once(s.to_string())
        .chain(rest.iter().map(|s| s.to_string()))
        .collect()
}

/// No env, no login → direct invocation, no shell wrapper.
#[test]
fn guest_invocation_direct_no_login() {
    let (prog, args) = build_guest_invocation(&cmd("htop"), &HashMap::new(), false);
    assert_eq!(prog, "htop");
    assert!(args.is_empty());
}

/// No env, no login, with args → direct invocation with args passed through.
#[test]
fn guest_invocation_direct_with_args() {
    let full_cmd = cmd_args("htop", &["--delay", "1"]);
    let (prog, args) = build_guest_invocation(&full_cmd, &HashMap::new(), false);
    assert_eq!(prog, "htop");
    assert_eq!(args, vec!["--delay".to_string(), "1".to_string()]);
}

/// Empty cmd, no env, no login → defaults to /bin/sh.
#[test]
fn guest_invocation_empty_cmd_defaults_to_sh() {
    let (prog, args) = build_guest_invocation(&[], &HashMap::new(), false);
    assert_eq!(prog, "/bin/sh");
    assert!(args.is_empty());
}

/// Login with no env → sh -l -c 'exec "$0" "$@"' cmd.
#[test]
fn guest_invocation_login_no_env() {
    let full_cmd = cmd_args("cargo", &["test"]);
    let (prog, args) = build_guest_invocation(&full_cmd, &HashMap::new(), true);
    assert_eq!(prog, "/bin/sh");
    assert_eq!(args[0], "-l");
    assert_eq!(args[1], "-c");
    assert!(
        args[2].contains("exec \"$0\" \"$@\""),
        "script should use exec: {}",
        args[2]
    );
    assert_eq!(args[3], "cargo");
    assert_eq!(args[4], "test");
}

/// Env vars are exported in the script.
#[test]
fn guest_invocation_env_no_login() {
    let env = HashMap::from([
        ("FOO".to_string(), "bar".to_string()),
        ("BAZ".to_string(), "qux".to_string()),
    ]);
    let (prog, args) = build_guest_invocation(&cmd("env"), &env, false);
    assert_eq!(prog, "/bin/sh");
    // No -l flag; first arg is -c
    assert_eq!(args[0], "-c");
    let script = &args[1];
    assert!(script.contains("export FOO='bar'"), "script: {script}");
    assert!(script.contains("export BAZ='qux'"), "script: {script}");
    assert!(script.contains("exec \"$0\" \"$@\""), "script: {script}");
    assert_eq!(args[2], "env");
}

/// Single quotes in env values are escaped correctly.
#[test]
fn guest_invocation_env_value_with_single_quote() {
    let env = HashMap::from([("MSG".to_string(), "it's a test".to_string())]);
    let (_, args) = build_guest_invocation(&cmd("sh"), &env, false);
    let script = &args[1];
    // "it's a test" → 'it'\''s a test'
    assert!(
        script.contains(r"'it'\''s a test'"),
        "expected escaped single quote in: {script}"
    );
}

/// Env + login: -l comes before -c.
#[test]
fn guest_invocation_env_with_login() {
    let env = HashMap::from([("KEY".to_string(), "val".to_string())]);
    let (prog, args) = build_guest_invocation(&cmd("sh"), &env, true);
    assert_eq!(prog, "/bin/sh");
    assert_eq!(args[0], "-l");
    assert_eq!(args[1], "-c");
    let script = &args[2];
    assert!(script.contains("export KEY='val'"));
    assert_eq!(args[3], "sh"); // cmd after script
}

// ── existing network tests ────────────────────────────────────────────────────

/// `public-only` builder output matches `NetworkPolicy::public_only()` exactly —
/// same defaults, same rule count, and same fields per rule.
#[test]
fn public_only_builtin_matches_sdk_preset() {
    let built = apply_named_policy(
        NetworkPolicy::builder(),
        "public-only",
        &HashMap::new(),
        None,
    )
    .unwrap()
    .build()
    .unwrap();
    let preset = NetworkPolicy::public_only();

    assert_eq!(built.default_egress, preset.default_egress);
    assert_eq!(built.default_ingress, preset.default_ingress);
    assert_eq!(
        built.rules.len(),
        preset.rules.len(),
        "rule count must match"
    );

    for (i, (built_rule, preset_rule)) in built.rules.iter().zip(preset.rules.iter()).enumerate() {
        assert_eq!(
            built_rule.direction, preset_rule.direction,
            "rule {i} direction"
        );
        assert_eq!(built_rule.action, preset_rule.action, "rule {i} action");
        assert_eq!(
            built_rule.protocols, preset_rule.protocols,
            "rule {i} protocols"
        );
        assert_eq!(built_rule.ports, preset_rule.ports, "rule {i} ports");
        // Compare Group destinations; other variants are not used by public_only()
        match (&built_rule.destination, &preset_rule.destination) {
            (
                microsandbox_network::policy::Destination::Group(bg),
                microsandbox_network::policy::Destination::Group(pg),
            ) => assert_eq!(bg, pg, "rule {i} destination group"),
            _ => panic!("rule {i}: expected Group destination"),
        }
    }
}

/// `apply_rule` with a valid domain rule produces a well-formed policy.
#[test]
fn apply_rule_domain_egress_builds_ok() {
    let rule = NetworkRule {
        direction: ConfigDirection::Egress,
        action: ConfigAction::Allow,
        destination: "api.example.com".to_string(),
        protocol: Some(ConfigProtocol::Tcp),
        ports: vec![443],
    };
    let policy = apply_rule(NetworkPolicy::builder(), &rule)
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(policy.rules.len(), 1);
    assert_eq!(policy.rules[0].direction, Direction::Egress);
    assert_eq!(policy.rules[0].action, Action::Allow);
    assert_eq!(policy.rules[0].protocols, vec![Protocol::Tcp]);
}

/// `*.example.com` wildcard destination maps to a `domain_suffix` rule (not a literal domain).
#[test]
fn apply_rule_wildcard_domain_uses_suffix() {
    let rule = NetworkRule {
        direction: ConfigDirection::Egress,
        action: ConfigAction::Allow,
        destination: "*.example.com".to_string(),
        protocol: Some(ConfigProtocol::Tcp),
        ports: vec![443],
    };
    let policy = apply_rule(NetworkPolicy::builder(), &rule)
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(policy.rules.len(), 1);
    assert_eq!(policy.rules[0].action, Action::Allow);
    // The destination should be a suffix match on "example.com", not the literal "*.example.com".
    let dest = format!("{:?}", policy.rules[0].destination);
    assert!(
        dest.contains("example.com") && !dest.contains("*.example.com"),
        "expected suffix destination, got: {dest}"
    );
}

/// `apply_rule` with a CIDR deny rule builds correctly.
#[test]
fn apply_rule_cidr_deny_builds_ok() {
    let rule = NetworkRule {
        direction: ConfigDirection::Any,
        action: ConfigAction::Deny,
        destination: "10.0.0.0/8".to_string(),
        protocol: None,
        ports: vec![],
    };
    let policy = apply_rule(NetworkPolicy::builder(), &rule)
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(policy.rules.len(), 1);
    assert_eq!(policy.rules[0].direction, Direction::Any);
    assert_eq!(policy.rules[0].action, Action::Deny);
}

/// Named policy from a file applies defaults + rules to the builder.
#[test]
fn apply_named_policy_from_file() {
    let named = NamedPolicy {
        default_egress: Some(ConfigAction::Deny),
        default_ingress: Some(ConfigAction::Allow),
        rules: vec![NetworkRule {
            direction: ConfigDirection::Egress,
            action: ConfigAction::Allow,
            destination: "public".to_string(),
            protocol: None,
            ports: vec![],
        }],
    };
    let policies = HashMap::from([("my-policy".to_string(), named)]);
    let policy = apply_named_policy(
        NetworkPolicy::builder(),
        "my-policy",
        &policies,
        Some(std::path::Path::new("/test/network-policy.d")),
    )
    .unwrap()
    .build()
    .unwrap();
    assert_eq!(policy.default_egress, Action::Deny);
    assert_eq!(policy.default_ingress, Action::Allow);
    assert_eq!(policy.rules.len(), 1);
    assert_eq!(policy.rules[0].action, Action::Allow);
    assert_eq!(policy.rules[0].direction, Direction::Egress);
}

/// Unknown policy name when no policies file → CONFIG_INVALID.
#[test]
fn apply_named_policy_unknown_builtin_returns_error() {
    let err = apply_named_policy(
        NetworkPolicy::builder(),
        "unknown-policy",
        &HashMap::new(),
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("unknown-policy"));
}

/// Unknown custom policy name when policies file exists → CONFIG_INVALID.
#[test]
fn apply_named_policy_unknown_in_file_returns_error() {
    let policies = HashMap::from([(
        "other-policy".to_string(),
        NamedPolicy {
            default_egress: None,
            default_ingress: None,
            rules: vec![],
        },
    )]);
    let err = apply_named_policy(
        NetworkPolicy::builder(),
        "my-missing-policy",
        &policies,
        Some(std::path::Path::new("/test/network-policy.d")),
    )
    .unwrap_err();
    assert_eq!(err.code, "CONFIG_INVALID");
    assert!(err.message.contains("my-missing-policy"));
}

/// Built-in names work even when a policies file is present.
#[test]
fn apply_named_policy_builtin_works_with_file_present() {
    let policies = HashMap::from([(
        "custom".to_string(),
        NamedPolicy {
            default_egress: None,
            default_ingress: None,
            rules: vec![],
        },
    )]);
    // All three built-ins should resolve even when a file is loaded.
    for name in ["none", "allow-all", "public-only"] {
        apply_named_policy(
            NetworkPolicy::builder(),
            name,
            &policies,
            Some(std::path::Path::new("/test/network-policy.d")),
        )
        .unwrap_or_else(|e| panic!("built-in '{name}' failed with file present: {e:?}"));
    }
}
