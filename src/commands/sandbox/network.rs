use std::collections::HashMap;

use crate::config::{ConfigAction, ConfigDirection, ConfigProtocol, NamedPolicy, NetworkRule};
use crate::error::SodagunError;
use microsandbox_network::policy::{Action, NetworkPolicyBuilder, RuleBuilder};

pub(super) fn to_sdk_action(action: ConfigAction) -> Action {
    match action {
        ConfigAction::Allow => Action::Allow,
        ConfigAction::Deny => Action::Deny,
    }
}

/// Resolve a named network policy. Built-in names (`none`, `allow-all`, `public-only`) are
/// always available and take priority. Custom policies are looked up in the loaded map.
pub(super) fn apply_named_policy(
    builder: NetworkPolicyBuilder,
    name: &str,
    policies: &HashMap<String, NamedPolicy>,
    policies_path: Option<&std::path::Path>,
) -> Result<NetworkPolicyBuilder, SodagunError> {
    // Built-ins are always resolved first; files in `network-policy.d/` cannot shadow them.
    match name {
        "none" => return Ok(builder.default_deny()),
        "allow-all" => return Ok(builder.default_allow()),
        // public-only: deny egress by default, allow ingress by default (the builder's empty
        // defaults already match: egress=Deny, ingress=Allow). Add DNS (UDP+TCP/53 to host
        // gateway) and public internet egress rules, mirroring NetworkPolicy::public_only().
        "public-only" => {
            return Ok(builder
                .egress(|e| e.udp().tcp().port(53).allow_host())
                .egress(|e| e.allow_public()));
        }
        _ => {}
    }
    let named = policies.get(name).ok_or_else(|| {
        let hint = match policies_path {
            Some(dir) => format!(
                "create {}/{name}.toml with the policy definition",
                dir.display()
            ),
            None => {
                "no network-policy.d/ directory found; built-ins are: none, allow-all, public-only"
                    .to_string()
            }
        };
        SodagunError {
            code: "CONFIG_INVALID",
            message: format!("unknown network policy '{name}'; {hint}"),
        }
    })?;
    let mut b = builder;
    if let Some(action) = named.default_egress {
        b = b.default_egress(to_sdk_action(action));
    }
    if let Some(action) = named.default_ingress {
        b = b.default_ingress(to_sdk_action(action));
    }
    for rule in &named.rules {
        b = apply_rule(b, rule)?;
    }
    Ok(b)
}

/// Apply a single [`NetworkRule`] to the policy builder using a `rule()` closure.
pub(super) fn apply_rule(
    builder: NetworkPolicyBuilder,
    rule: &NetworkRule,
) -> Result<NetworkPolicyBuilder, SodagunError> {
    let dir = rule.direction;
    let action = rule.action;
    let dest = rule.destination.clone();
    let protocol = rule.protocol;
    let ports = rule.ports.clone();

    // Use .rule() and set direction inside the closure to avoid needing multiple closures.
    Ok(builder.rule(move |rb| {
        match dir {
            ConfigDirection::Egress => {
                rb.egress();
            }
            ConfigDirection::Ingress => {
                rb.ingress();
            }
            ConfigDirection::Any => {
                rb.any();
            }
        }
        match protocol {
            Some(ConfigProtocol::Tcp) => {
                rb.tcp();
            }
            Some(ConfigProtocol::Udp) => {
                rb.udp();
            }
            None => {}
        }
        for &p in &ports {
            rb.port(p);
        }
        commit_dest(rb, action, &dest)
    }))
}

/// Commit a rule destination + action onto a [`RuleBuilder`], returning it.
pub(super) fn commit_dest<'a>(
    rb: &'a mut RuleBuilder,
    action: ConfigAction,
    destination: &str,
) -> &'a mut RuleBuilder {
    match (action, destination) {
        (ConfigAction::Allow, "public") => rb.allow_public(),
        (ConfigAction::Deny, "public") => rb.deny_public(),
        (ConfigAction::Allow, "private") => rb.allow_private(),
        (ConfigAction::Deny, "private") => rb.deny_private(),
        (ConfigAction::Allow, "host") => rb.allow_host(),
        (ConfigAction::Deny, "host") => rb.deny_host(),
        (ConfigAction::Allow, "loopback") => rb.allow_loopback(),
        (ConfigAction::Deny, "loopback") => rb.deny_loopback(),
        (ConfigAction::Allow, "link_local") => rb.allow_link_local(),
        (ConfigAction::Deny, "link_local") => rb.deny_link_local(),
        (ConfigAction::Allow, "metadata") => rb.allow_meta(),
        (ConfigAction::Deny, "metadata") => rb.deny_meta(),
        (ConfigAction::Allow, "multicast") => rb.allow_multicast(),
        (ConfigAction::Deny, "multicast") => rb.deny_multicast(),
        _ => match action {
            ConfigAction::Allow => {
                if destination == "any" {
                    rb.allow().any()
                } else if destination.contains('/') {
                    rb.allow().cidr(destination)
                } else if destination.parse::<std::net::IpAddr>().is_ok() {
                    rb.allow().ip(destination)
                } else if let Some(suffix) = destination.strip_prefix("*.") {
                    // *.example.com → domain_suffix matches apex + all subdomains
                    rb.allow().domain_suffix(suffix)
                } else {
                    rb.allow().domain(destination)
                }
            }
            ConfigAction::Deny => {
                if destination == "any" {
                    rb.deny().any()
                } else if destination.contains('/') {
                    rb.deny().cidr(destination)
                } else if destination.parse::<std::net::IpAddr>().is_ok() {
                    rb.deny().ip(destination)
                } else if let Some(suffix) = destination.strip_prefix("*.") {
                    rb.deny().domain_suffix(suffix)
                } else {
                    rb.deny().domain(destination)
                }
            }
        },
    }
}
