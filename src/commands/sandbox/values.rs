use crate::config::{EnvValue, SecretConfig, ValueSource};
use crate::context::Context;
use crate::error::SodagunError;

/// Run `sh -c <cmd>` on the host, return trimmed stdout. Used by both env and secret resolution.
pub(super) fn run_value_cmd(
    ctx: Context,
    var_name: &str,
    cmd: &str,
) -> Result<String, SodagunError> {
    ctx.log(&format!("'{var_name}': running value_from_cmd: {cmd}"));
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map_err(|e| SodagunError {
            code: "CONFIG_INVALID",
            message: format!("'{var_name}': failed to run value_from_cmd: {e}"),
        })?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        return Err(SodagunError {
            code: "CONFIG_INVALID",
            message: format!("'{var_name}': value_from_cmd exited with code {code}: {cmd}"),
        });
    }
    let value = String::from_utf8(output.stdout).map_err(|_| SodagunError {
        code: "CONFIG_INVALID",
        message: format!("'{var_name}': value_from_cmd output is not valid UTF-8"),
    })?;
    Ok(value.trim_end().to_string())
}

/// Reject values that contain control characters (`\n`, `\r`, NUL, etc.).
/// Both env vars and secrets must be single-line plain text; passing control characters
/// to the microsandbox SDK causes the VM to SIGABRT before the agent relay starts.
pub(super) fn validate_value_str(label: &str, value: &str) -> Result<(), SodagunError> {
    if let Some(bad) = value.chars().find(|c| c.is_control()) {
        return Err(SodagunError {
            code: "CONFIG_INVALID",
            message: format!(
                "'{label}': value contains a control character ({bad:?}); \
                 values must be single-line plain text; got: {value:?}"
            ),
        });
    }
    Ok(())
}

/// Resolve a `ValueSource` (the dynamic form of `EnvValue`).
pub(super) fn resolve_value_source(
    ctx: Context,
    var_name: &str,
    src: &ValueSource,
) -> Result<String, SodagunError> {
    match (&src.value, &src.value_from_env, &src.value_from_cmd) {
        (Some(literal), None, None) => Ok(literal.clone()),
        (None, Some(from_env), None) => std::env::var(from_env).map_err(|_| SodagunError {
            code: "CONFIG_INVALID",
            message: format!("'{var_name}' references env var '{from_env}' which is not set"),
        }),
        (None, None, Some(cmd)) => run_value_cmd(ctx, var_name, cmd),
        _ => Err(SodagunError {
            code: "CONFIG_INVALID",
            message: format!(
                "'{var_name}' must set exactly one of 'value', 'value_from_env', or 'value_from_cmd'"
            ),
        }),
    }
}

/// Resolve an `EnvValue` to a plain string at launch time.
pub(super) fn resolve_env_value(
    ctx: Context,
    var_name: &str,
    val: &EnvValue,
) -> Result<String, SodagunError> {
    let resolved = match val {
        EnvValue::Literal(s) => s.clone(),
        EnvValue::Dynamic(src) => resolve_value_source(ctx, var_name, src)?,
    };
    validate_value_str(var_name, &resolved)?;
    Ok(resolved)
}

/// Resolve a secret's value from `value`, `value_from_env`, or `value_from_cmd`.
pub(super) fn resolve_secret_value(
    ctx: Context,
    env_var: &str,
    secret: &SecretConfig,
) -> Result<String, SodagunError> {
    match (
        &secret.value,
        &secret.value_from_env,
        &secret.value_from_cmd,
    ) {
        (Some(literal), None, None) => Ok(literal.clone()),
        (None, Some(from_env), None) => std::env::var(from_env).map_err(|_| SodagunError {
            code: "CONFIG_INVALID",
            message: format!("secret '{env_var}' references env var '{from_env}' which is not set"),
        }),
        (None, None, Some(cmd)) => run_value_cmd(ctx, env_var, cmd),
        _ => Err(SodagunError {
            code: "CONFIG_INVALID",
            message: format!(
                "secret '{env_var}' must set exactly one of 'value', 'value_from_env', or 'value_from_cmd'"
            ),
        }),
    }
    .and_then(|v| {
        validate_value_str(env_var, &v)?;
        Ok(v)
    })
}
