use std::collections::HashMap;

use crate::config::{ConfigAction, ConfigDirection, ConfigProtocol, NamedPolicy, NetworkRule};
use microsandbox::NetworkPolicy;
use microsandbox_network::policy::{Action, Direction, Protocol};

use super::network::{apply_named_policy, apply_rule};

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
        Some(std::path::Path::new("/test/network-policies.toml")),
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
        Some(std::path::Path::new("/test/network-policies.toml")),
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
            Some(std::path::Path::new("/test/network-policies.toml")),
        )
        .unwrap_or_else(|e| panic!("built-in '{name}' failed with file present: {e:?}"));
    }
}
