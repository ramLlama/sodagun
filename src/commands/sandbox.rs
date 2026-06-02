use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::config::{
    ConfigAction, ConfigDirection, ConfigProtocol, EnvValue, ImageConfig, NamedPolicy, NetworkRule,
    RawSandboxConfig, SandboxConfig, ValueSource, load_network_policies, load_user_sandbox_config,
    merge_sandbox_configs, parse_volume,
};
use crate::context::{Context, OutputFormat};
use crate::error::{SodagunError, handle_error};
use crate::util;
use crate::workspace::WorkspaceMetadata;
use clap::{Parser, Subcommand};
use microsandbox::{MicrosandboxError, NetworkPolicy, Sandbox, Snapshot};
use microsandbox_network::policy::{Action, NetworkPolicyBuilder, RuleBuilder};

/// Name prefix shared by all sodagun-created sandboxes: worktree sandboxes
/// (`sodagun_<repo>_<branch>_<uuid>`) and ephemeral snapshot builders
/// (`sodagun-snap-<uuid>`). Used to filter `sandbox list` to our own VMs.
const SODAGUN_PREFIX: &str = "sodagun";

#[derive(Parser)]
pub struct SandboxCommand {
    #[command(subcommand)]
    pub subcommand: SandboxSubcommand,
}

#[derive(Subcommand)]
pub enum SandboxSubcommand {
    /// Start a sandbox for a worktree.
    Start(StartArgs),
    /// Attach an interactive TTY session to a running sandbox.
    Attach(AttachArgs),
    /// Run a command in a running sandbox and return its output.
    Exec(ExecArgs),
    /// List all sandboxes and their statuses.
    List(ListArgs),
    /// Stop a running sandbox.
    Stop(StopArgs),
    /// Stop and remove a sandbox.
    Remove(RemoveArgs),
}

#[derive(Parser)]
pub struct StartArgs {
    /// Workspace rootdir created by `git add-worktree`.
    pub workspace_path: PathBuf,

    /// Path to the sodagun config file (default: <worktree-path>/sodagun.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(Parser)]
pub struct AttachArgs {
    /// Workspace rootdir of the sandbox to attach to.
    pub workspace_path: PathBuf,

    /// Skip login shell; attach without sourcing profile files.
    #[arg(long)]
    pub no_login: bool,
}

#[derive(Parser)]
pub struct ExecArgs {
    /// Workspace rootdir of the sandbox to exec into.
    pub workspace_path: PathBuf,

    /// Command to run inside the sandbox.
    pub cmd: String,

    /// Arguments for the command.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,

    /// Skip login shell; run the command directly without sourcing profile files.
    #[arg(long)]
    pub no_login: bool,
}

#[derive(Parser)]
pub struct ListArgs {}

#[derive(Parser)]
pub struct StopArgs {
    /// Workspace rootdir of the sandbox to stop.
    pub workspace_path: PathBuf,

    /// Seconds to wait for the sandbox to reach stopped state (default: 30).
    #[arg(long, default_value_t = 30)]
    pub stop_timeout_seconds: u64,

    /// Send the stop signal and return immediately without waiting.
    #[arg(long)]
    pub no_wait: bool,
}

#[derive(Parser)]
pub struct RemoveArgs {
    /// Workspace rootdir of the sandbox to remove.
    pub workspace_path: PathBuf,

    /// Seconds to wait for the sandbox to stop before removing (default: 30).
    #[arg(long, default_value_t = 30)]
    pub stop_timeout_seconds: u64,
}

pub fn run(ctx: Context, cmd: SandboxCommand) {
    match cmd.subcommand {
        SandboxSubcommand::Start(args) => start(ctx, args),
        SandboxSubcommand::Attach(args) => attach(ctx, args),
        SandboxSubcommand::Exec(args) => exec(ctx, args),
        SandboxSubcommand::List(args) => list(ctx, args),
        SandboxSubcommand::Stop(args) => stop(ctx, args),
        SandboxSubcommand::Remove(args) => remove(ctx, args),
    }
}

/// Read the sandbox name from a workspace's sodagun.json, exiting with the
/// appropriate error code if the workspace is missing or has no sandbox started.
fn read_sandbox_name(ctx: Context, rootdir: &std::path::Path) -> String {
    let meta = WorkspaceMetadata::read(rootdir).unwrap_or_else(|e| handle_error(ctx, e));
    meta.sandbox_name.unwrap_or_else(|| {
        handle_error(
            ctx,
            SodagunError {
                code: "SANDBOX_NOT_STARTED",
                message: format!(
                    "no sandbox has been started for this workspace: {}",
                    rootdir.display()
                ),
            },
        )
    })
}

fn start(ctx: Context, args: StartArgs) {
    let meta =
        WorkspaceMetadata::read(&args.workspace_path).unwrap_or_else(|e| handle_error(ctx, e));

    if let Some(ref name) = meta.sandbox_name {
        handle_error(
            ctx,
            SodagunError {
                code: "SANDBOX_ALREADY_STARTED",
                message: format!(
                    "sandbox '{name}' is already started for this workspace; stop or remove it first"
                ),
            },
        );
    }

    if !meta.worktree_path.is_dir() {
        handle_error(
            ctx,
            SodagunError {
                code: "WORKTREE_NOT_FOUND",
                message: format!(
                    "worktree path does not exist or is not a directory: {}",
                    meta.worktree_path.display()
                ),
            },
        );
    }

    // Config resolution: explicit --config > worktree/sodagun.toml > repo/sodagun.toml > defaults.
    // The fallback to repo_path lets branches that haven't added a per-branch config yet
    // inherit the project-level config.
    let resolved_config: Option<std::path::PathBuf> = if let Some(path) = args.config {
        Some(path)
    } else {
        let worktree_toml = meta.worktree_path.join("sodagun.toml");
        let repo_toml = meta.repo_path.join("sodagun.toml");
        if worktree_toml.exists() {
            Some(worktree_toml)
        } else if repo_toml.exists() {
            ctx.log(&format!(
                "no sodagun.toml in worktree; using project config from {}",
                repo_toml.display()
            ));
            Some(repo_toml)
        } else {
            None
        }
    };

    let (image_config, raw_project) = match resolved_config {
        Some(path) => match crate::config::load_config(&path) {
            Ok(pair) => pair,
            Err(e) => handle_error(ctx, e),
        },
        // No config anywhere; use conservative defaults (alpine:latest, airgapped, etc.)
        None => (
            crate::config::default_image_config(),
            RawSandboxConfig::default(),
        ),
    };

    let user_sandbox = match load_user_sandbox_config() {
        Ok(s) => s,
        Err(e) => handle_error(ctx, e),
    };
    let (network_policies, policies_path) = match load_network_policies() {
        Ok(p) => p,
        Err(e) => handle_error(ctx, e),
    };
    let sandbox_config = match merge_sandbox_configs(user_sandbox, raw_project) {
        Ok(s) => s,
        Err(e) => handle_error(ctx, e),
    };

    // Sandbox name == workspace directory name, enforcing a strict 1:1 worktree↔VM mapping.
    let sandbox_name = args
        .workspace_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            handle_error(
                ctx,
                SodagunError {
                    code: "WORKSPACE_INVALID",
                    message: format!(
                        "workspace path has no directory name: {}",
                        args.workspace_path.display()
                    ),
                },
            )
        });

    // Reserve the sandbox name in metadata BEFORE launching. This way, if the process is
    // interrupted after launch succeeds but before a post-launch write, the name is already
    // persisted and the sandbox is still reachable. If launch fails, we clear it as rollback.
    if let Err(e) =
        WorkspaceMetadata::set_sandbox_name(&args.workspace_path, Some(sandbox_name.clone()))
    {
        handle_error(ctx, e);
    }

    let rt = util::get_runtime();
    let name = match rt.block_on(start_async(
        ctx,
        &sandbox_name,
        &meta.worktree_path,
        &image_config,
        &sandbox_config,
        &network_policies,
        policies_path.as_deref(),
    )) {
        Ok(n) => n,
        Err(e) => {
            // Best-effort rollback: clear the name we reserved so the workspace is reusable
            let _ = WorkspaceMetadata::set_sandbox_name(&args.workspace_path, None);
            handle_error(ctx, e)
        }
    };

    match ctx.output {
        OutputFormat::Text => println!("{}", name),
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({ "status": "ok", "sandbox_name": name })
            )
        }
    }
}

fn attach(ctx: Context, args: AttachArgs) {
    let sandbox_name = read_sandbox_name(ctx, &args.workspace_path);

    let rt = util::get_runtime();
    match rt.block_on(attach_async(&sandbox_name, !args.no_login)) {
        Ok(exit_code) => std::process::exit(exit_code),
        Err(e) => handle_error(ctx, e),
    }
}

fn exec(ctx: Context, args: ExecArgs) {
    let sandbox_name = read_sandbox_name(ctx, &args.workspace_path);

    let rt = util::get_runtime();
    match rt.block_on(exec_async(
        &sandbox_name,
        &args.cmd,
        &args.args,
        !args.no_login,
    )) {
        Ok(output) => {
            let exit_code = output.status().code;
            match ctx.output {
                OutputFormat::Text => {
                    // Write captured stdout/stderr to the corresponding streams.
                    use std::io::Write;
                    let _ = std::io::stdout().write_all(output.stdout_bytes());
                    let _ = std::io::stderr().write_all(output.stderr_bytes());
                }
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "status": "ok",
                            "exit_code": exit_code,
                            "stdout": output.stdout().unwrap_or_default(),
                            "stderr": output.stderr().unwrap_or_default(),
                        })
                    );
                }
            }
            std::process::exit(exit_code);
        }
        Err(e) => handle_error(ctx, e),
    }
}

fn list(ctx: Context, _args: ListArgs) {
    let rt = util::get_runtime();
    let (sandboxes, hidden) = match rt.block_on(list_async()) {
        Ok(s) => s,
        Err(e) => handle_error(ctx, e),
    };

    match ctx.output {
        OutputFormat::Text => {
            let name_width = sandboxes
                .iter()
                .map(|(n, _)| n.len())
                .max()
                .unwrap_or(0)
                .max("NAME".len());
            println!("{:<width$}  STATUS", "NAME", width = name_width);
            for (name, status) in &sandboxes {
                println!("{:<width$}  {}", name, status, width = name_width);
            }
        }
        OutputFormat::Json => {
            let items: Vec<_> = sandboxes
                .iter()
                .map(|(name, status)| serde_json::json!({"name": name, "status": status}))
                .collect();
            println!(
                "{}",
                serde_json::json!({"status": "ok", "sandboxes": items})
            );
        }
    }

    // Note (to stderr, so JSON on stdout stays clean) when non-sodagun VMs were hidden.
    if hidden > 0 {
        ctx.log(&format!(
            "{hidden} non-sodagun sandbox(es) hidden; run `msb list` to see all microsandbox VMs"
        ));
    }
}

fn stop(ctx: Context, args: StopArgs) {
    let sandbox_name = read_sandbox_name(ctx, &args.workspace_path);

    let rt = util::get_runtime();
    let timeout = Duration::from_secs(args.stop_timeout_seconds);
    match rt.block_on(stop_async(&sandbox_name, timeout, args.no_wait)) {
        Ok(()) => {}
        Err(e) => handle_error(ctx, e),
    }

    match ctx.output {
        OutputFormat::Text => {
            if args.no_wait {
                println!("Stop signal sent.");
            } else {
                println!("Stopped.");
            }
        }
        OutputFormat::Json => println!("{}", serde_json::json!({"status": "ok"})),
    }
}

fn remove(ctx: Context, args: RemoveArgs) {
    let sandbox_name = read_sandbox_name(ctx, &args.workspace_path);

    let rt = util::get_runtime();
    let timeout = Duration::from_secs(args.stop_timeout_seconds);
    match rt.block_on(remove_async(&sandbox_name, timeout)) {
        Ok(()) => {}
        Err(e) => handle_error(ctx, e),
    }

    // Clear sandbox_name now that the sandbox has been removed
    if let Err(e) = WorkspaceMetadata::set_sandbox_name(&args.workspace_path, None) {
        handle_error(ctx, e);
    }

    match ctx.output {
        OutputFormat::Text => println!("Removed."),
        OutputFormat::Json => println!("{}", serde_json::json!({"status": "ok"})),
    }
}

/// Polls `Sandbox::get` every 500ms until the sandbox reaches a terminal status
/// (Stopped or Crashed), or until `timeout` elapses. Checks status before sleeping
/// so fast-stopping sandboxes are detected immediately.
async fn poll_until_stopped(name: &str, timeout: Duration) -> Result<(), SodagunError> {
    let deadline = Instant::now() + timeout;
    loop {
        let handle = Sandbox::get(name)
            .await
            .map_err(|e| util::map_sandbox_err(e, name))?;
        if util::is_terminal_status(handle.status()) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(SodagunError {
                code: "SANDBOX_ERROR",
                message: format!(
                    "timed out waiting for sandbox '{name}' to stop after {}s",
                    timeout.as_secs()
                ),
            });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Lists sodagun-managed sandboxes (those whose name starts with [`SODAGUN_PREFIX`]),
/// returning them alongside the count of other microsandbox VMs filtered out.
async fn list_async() -> Result<(Vec<(String, String)>, usize), SodagunError> {
    let handles = Sandbox::list().await.map_err(|e| SodagunError {
        code: "SANDBOX_ERROR",
        message: format!("failed to list sandboxes: {e}"),
    })?;
    let total = handles.len();
    let sandboxes: Vec<(String, String)> = handles
        .into_iter()
        .filter(|h| h.name().starts_with(SODAGUN_PREFIX))
        .map(|h| {
            (
                h.name().to_string(),
                util::status_label(h.status()).to_string(),
            )
        })
        .collect();
    let hidden = total - sandboxes.len();
    Ok((sandboxes, hidden))
}

async fn stop_async(name: &str, timeout: Duration, no_wait: bool) -> Result<(), SodagunError> {
    let handle = Sandbox::get(name)
        .await
        .map_err(|e| util::map_sandbox_err(e, name))?;
    // Already terminal — stop is a no-op.
    if util::is_terminal_status(handle.status()) {
        return Ok(());
    }
    handle.stop().await.map_err(|e| SodagunError {
        code: "SANDBOX_ERROR",
        message: format!("failed to send stop signal to '{name}': {e}"),
    })?;
    if !no_wait {
        poll_until_stopped(name, timeout).await?;
    }
    Ok(())
}

async fn remove_async(name: &str, timeout: Duration) -> Result<(), SodagunError> {
    let handle = Sandbox::get(name)
        .await
        .map_err(|e| util::map_sandbox_err(e, name))?;

    // Implicitly stop if still running before attempting removal.
    if !util::is_terminal_status(handle.status()) {
        handle.stop().await.map_err(|e| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("failed to send stop signal to '{name}': {e}"),
        })?;
        poll_until_stopped(name, timeout).await?;
    }

    Sandbox::remove(name)
        .await
        .map_err(|e| util::map_sandbox_err(e, name))
}

async fn start_async(
    ctx: Context,
    sandbox_name: &str,
    worktree_path: &std::path::Path,
    image_config: &ImageConfig,
    sandbox_config: &SandboxConfig,
    network_policies: &HashMap<String, NamedPolicy>,
    // Some(path) when the user's network-policies.toml file exists; None → use built-ins.
    policies_path: Option<&std::path::Path>,
) -> Result<String, SodagunError> {
    let mut builder = Sandbox::builder(sandbox_name);

    // Determine what to boot from. When a setup script is configured, we boot from
    // the derived snapshot; the snapshot must already exist (run `sodagun snapshot create`).
    if let Some(snap_name) = image_config.derived_snapshot_name() {
        Snapshot::get(&snap_name).await.map_err(|e| {
            if matches!(e, MicrosandboxError::SnapshotNotFound(_)) {
                SodagunError {
                    code: "SNAPSHOT_NOT_FOUND",
                    message: format!(
                        "snapshot '{snap_name}' not found — run 'sodagun snapshot create <rootdir>' first"
                    ),
                }
            } else {
                SodagunError {
                    code: "SNAPSHOT_ERROR",
                    message: format!("{e}"),
                }
            }
        })?;
        ctx.log(&format!("booting from project snapshot: {snap_name}"));
        builder = builder.from_snapshot(&snap_name);
    } else if let Some(ref image) = image_config.base_image {
        ctx.log(&format!("booting from image: {image}"));
        builder = builder.image(image.as_str());
    } else if let Some(ref snapshot) = image_config.base_snapshot {
        ctx.log(&format!("booting from snapshot: {snapshot}"));
        builder = builder.from_snapshot(snapshot.as_str());
    }

    builder = builder
        .cpus(sandbox_config.cpus)
        .memory(sandbox_config.memory_mb)
        .workdir(&sandbox_config.working_dir);

    // Network: no config at all → disable (safe default); otherwise build a policy.
    let net = &sandbox_config.network;
    let has_network_config = net.policy.is_some()
        || !net.rules.is_empty()
        || net.default_egress.is_some()
        || net.default_ingress.is_some();

    if !has_network_config {
        builder = builder.disable_network();
    } else {
        let mut policy_builder = NetworkPolicy::builder();

        // Apply named policy base (built-in or user-defined)
        if let Some(ref name) = net.policy {
            policy_builder =
                apply_named_policy(policy_builder, name, network_policies, policies_path)?;
        }

        // Apply inline rules (user + project already merged; user first)
        for rule in &net.rules {
            policy_builder = apply_rule(policy_builder, rule)?;
        }

        // Inline defaults override named-policy defaults (last-write-wins in the builder)
        if let Some(action) = net.default_egress {
            policy_builder = policy_builder.default_egress(to_sdk_action(action));
        }
        if let Some(action) = net.default_ingress {
            policy_builder = policy_builder.default_ingress(to_sdk_action(action));
        }

        let policy = policy_builder.build().map_err(|e| SodagunError {
            code: "CONFIG_INVALID",
            message: format!("invalid network policy: {e}"),
        })?;
        builder = builder.network(|b| b.policy(policy));
    }

    // Bind-mount the worktree at the configured working_dir
    let worktree_str = worktree_path.to_str().ok_or_else(|| SodagunError {
        code: "CONFIG_INVALID",
        message: "worktree path contains non-UTF-8 characters".to_string(),
    })?;
    builder = builder.volume(&sandbox_config.working_dir, |m| m.bind(worktree_str));

    // Additional volumes declared in config
    for vol_str in &sandbox_config.volumes {
        let (host_path, guest_path, readonly) = parse_volume(vol_str)?;
        let host_str = host_path
            .to_str()
            .ok_or_else(|| SodagunError {
                code: "CONFIG_INVALID",
                message: format!("volume host path is non-UTF-8: {}", host_path.display()),
            })?
            .to_owned();
        if readonly {
            builder = builder.volume(guest_path, move |m| m.bind(&host_str).readonly());
        } else {
            builder = builder.volume(guest_path, move |m| m.bind(&host_str));
        }
    }

    // Env vars — resolve any dynamic sources (value_from_env, value_from_cmd) at launch time
    let resolved_env = sandbox_config
        .env
        .iter()
        .map(|(k, v)| resolve_env_value(ctx, k, v).map(|s| (k.clone(), s)))
        .collect::<Result<Vec<_>, _>>()?;
    builder = builder.envs(resolved_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));

    // Secrets — resolve value_from_env / value_from_cmd at launch time
    for (env_var, secret) in &sandbox_config.secrets {
        let value = resolve_secret_value(ctx, env_var, secret)?;

        let env_var_owned = env_var.clone();
        let allowed_hosts = secret.allowed_hosts.clone();
        builder = builder.secret(move |s| {
            let mut s = s.env(&env_var_owned).value(value);
            for host in &allowed_hosts {
                if host.contains('*') {
                    s = s.allow_host_pattern(host);
                } else {
                    s = s.allow_host(host);
                }
            }
            s
        });
    }

    let sandbox = builder.create_detached().await.map_err(|e| SodagunError {
        code: "SANDBOX_ERROR",
        message: format!("failed to create sandbox: {e}"),
    })?;

    Ok(sandbox.name().to_string())
}

/// Run `sh -c <cmd>` on the host, return trimmed stdout. Used by both env and secret resolution.
fn run_value_cmd(ctx: Context, var_name: &str, cmd: &str) -> Result<String, SodagunError> {
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
fn validate_value_str(label: &str, value: &str) -> Result<(), SodagunError> {
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
fn resolve_value_source(
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
fn resolve_env_value(ctx: Context, var_name: &str, val: &EnvValue) -> Result<String, SodagunError> {
    let resolved = match val {
        EnvValue::Literal(s) => s.clone(),
        EnvValue::Dynamic(src) => resolve_value_source(ctx, var_name, src)?,
    };
    validate_value_str(var_name, &resolved)?;
    Ok(resolved)
}

/// Resolve a secret's value from `value`, `value_from_env`, or `value_from_cmd`.
fn resolve_secret_value(
    ctx: Context,
    env_var: &str,
    secret: &crate::config::SecretConfig,
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

fn to_sdk_action(action: ConfigAction) -> Action {
    match action {
        ConfigAction::Allow => Action::Allow,
        ConfigAction::Deny => Action::Deny,
    }
}

/// Resolve a named network policy. Built-in names (`none`, `allow-all`, `public-only`) are
/// always available and take priority. Custom policies are looked up in the loaded map.
fn apply_named_policy(
    builder: NetworkPolicyBuilder,
    name: &str,
    policies: &HashMap<String, NamedPolicy>,
    policies_path: Option<&std::path::Path>,
) -> Result<NetworkPolicyBuilder, SodagunError> {
    // Built-ins are always resolved first; `network-policies.toml` cannot shadow them.
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
            Some(path) => format!("define it in {}", path.display()),
            None => "no network-policies.toml found; built-ins are: none, allow-all, public-only"
                .to_string(),
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
fn apply_rule(
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
fn commit_dest<'a>(
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
                } else {
                    rb.deny().domain(destination)
                }
            }
        },
    }
}

async fn exec_async(
    sandbox_name: &str,
    cmd: &str,
    args: &[String],
    login: bool,
) -> Result<microsandbox::sandbox::ExecOutput, SodagunError> {
    let sandbox = Sandbox::start(sandbox_name)
        .await
        .map_err(|e| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("failed to connect to sandbox '{sandbox_name}': {e}"),
        })?;

    if login {
        // Run through a login shell so profile files (and PATH, e.g. /root/.cargo/bin)
        // are sourced. `sh -l -c 'exec "$0" "$@"' <cmd> <args>` sources the profiles
        // then `exec` *replaces* the shell in place with the real command — no nested
        // shell — preserving argv exactly without re-quoting (cmd is $0, args are $@).
        let login_args: Vec<&str> = ["-l", "-c", "exec \"$0\" \"$@\"", cmd]
            .iter()
            .copied()
            .chain(args.iter().map(String::as_str))
            .collect();
        sandbox
            .exec("/bin/sh", login_args)
            .await
            .map_err(|e| SodagunError {
                code: "SANDBOX_ERROR",
                message: format!("exec failed in sandbox '{sandbox_name}': {e}"),
            })
    } else {
        sandbox
            .exec(cmd, args.iter().map(String::as_str))
            .await
            .map_err(|e| SodagunError {
                code: "SANDBOX_ERROR",
                message: format!("exec failed in sandbox '{sandbox_name}': {e}"),
            })
    }
}

/// Returns the shell's exit code on a normal interactive session end.
/// Returns `Err` only on infrastructure failure (connection lost, etc.).
async fn attach_async(sandbox_name: &str, login: bool) -> Result<i32, SodagunError> {
    let sandbox = Sandbox::start(sandbox_name)
        .await
        .map_err(|e| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("failed to connect to sandbox '{sandbox_name}': {e}"),
        })?;

    if login {
        sandbox
            .attach("/bin/sh", ["-l"])
            .await
            .map_err(|e| SodagunError {
                code: "SANDBOX_ERROR",
                message: format!("attach session failed: {e}"),
            })
    } else {
        sandbox.attach_shell().await.map_err(|e| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("attach session failed: {e}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use microsandbox_network::policy::{Action, Direction, Protocol};

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

        for (i, (built_rule, preset_rule)) in
            built.rules.iter().zip(preset.rules.iter()).enumerate()
        {
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
        use crate::config::{ConfigAction, ConfigDirection, NetworkRule};
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

    /// `apply_rule` with a CIDR deny rule builds correctly.
    #[test]
    fn apply_rule_cidr_deny_builds_ok() {
        use crate::config::{ConfigAction, ConfigDirection, NetworkRule};
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
        use crate::config::{ConfigAction, ConfigDirection, NamedPolicy, NetworkRule};
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
        use crate::config::NamedPolicy;
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
        use crate::config::NamedPolicy;
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
}
