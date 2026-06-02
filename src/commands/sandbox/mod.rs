use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::{
    ImageConfig, NamedPolicy, RawSandboxConfig, RegistryConfig, SandboxConfig,
    load_network_policies, load_registry_config, load_user_image_config, load_user_registry_config,
    load_user_sandbox_config, merge_registry_configs, merge_sandbox_configs,
    merge_user_image_config, parse_volume,
};
use crate::context::{Context, OutputFormat};
use crate::error::{SodagunError, handle_error};
use crate::util;
use crate::workspace::WorkspaceMetadata;
use clap::{Parser, Subcommand};
use microsandbox::{MicrosandboxError, NetworkPolicy, Sandbox};

mod network;
mod values;
use self::network::{apply_named_policy, apply_rule, to_sdk_action};
use self::values::{resolve_env_value, resolve_secret_value};

#[cfg(test)]
mod tests;

/// Name prefix shared by all sodagun-created sandboxes (`sodagun_<repo>_<branch>_<uuid>`).
/// Used to filter `sandbox list` to our own VMs.
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
    /// Build and push a Dockerfile-based image for this project.
    CreateImage(CreateImageArgs),
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

#[derive(Parser)]
pub struct CreateImageArgs {
    /// Path to the config file (default: <project-dir>/sodagun.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Force rebuild even if the image tag already exists in the registry.
    #[arg(long)]
    pub force: bool,
}

pub fn run(ctx: Context, cmd: SandboxCommand, project_dir: PathBuf) {
    match cmd.subcommand {
        SandboxSubcommand::Start(args) => start(ctx, args),
        SandboxSubcommand::Attach(args) => attach(ctx, args),
        SandboxSubcommand::Exec(args) => exec(ctx, args),
        SandboxSubcommand::List(args) => list(ctx, args),
        SandboxSubcommand::Stop(args) => stop(ctx, args),
        SandboxSubcommand::Remove(args) => remove(ctx, args),
        SandboxSubcommand::CreateImage(args) => create_image(ctx, args, project_dir),
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

    let (image_config, raw_project) = match &resolved_config {
        Some(path) => match crate::config::load_config(path) {
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

    // Load and merge registry + user image overrides (needed for dockerfile path).
    // Pass resolved_config so --config overrides are honored (not re-derived).
    let (image_config, registry) =
        load_and_merge_image_overrides(ctx, &resolved_config, image_config);

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

    // Compute the OCI tag if a dockerfile is configured, so we can pass it into start_async.
    let dockerfile_tag = if let Some(ref df_path) = image_config.dockerfile {
        let bytes = std::fs::read(df_path).unwrap_or_else(|e| {
            handle_error(
                ctx,
                SodagunError {
                    code: "CONFIG_INVALID",
                    message: format!("failed to read dockerfile '{}': {e}", df_path.display()),
                },
            )
        });
        let tag = match crate::config::dockerfile_image_tag(&image_config, &registry, &bytes) {
            Ok(t) => t,
            Err(e) => handle_error(ctx, e),
        };
        Some(tag)
    } else {
        None
    };

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
        &registry,
        dockerfile_tag.as_deref(),
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

/// Load registry config and merge user image overrides into `image_config`.
/// Returns (merged ImageConfig, merged RegistryConfig).
///
/// `config_path` is the resolved project config path (or None if defaults were used).
fn load_and_merge_image_overrides(
    ctx: Context,
    config_path: &Option<PathBuf>,
    image_config: ImageConfig,
) -> (ImageConfig, RegistryConfig) {
    let proj_registry = match config_path {
        Some(path) => match load_registry_config(path) {
            Ok(r) => r,
            Err(e) => handle_error(ctx, e),
        },
        None => RegistryConfig::default(),
    };
    let user_registry = match load_user_registry_config() {
        Ok(r) => r,
        Err(e) => handle_error(ctx, e),
    };
    let registry = merge_registry_configs(user_registry, proj_registry);

    let user_image = match load_user_image_config() {
        Ok(u) => u,
        Err(e) => handle_error(ctx, e),
    };
    let image_config = merge_user_image_config(user_image, image_config);

    (image_config, registry)
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

fn create_image(ctx: Context, args: CreateImageArgs, project_dir: PathBuf) {
    let config_path = args
        .config
        .unwrap_or_else(|| project_dir.join("sodagun.toml"));

    let image_config = match crate::config::load_config(&config_path) {
        Ok((img, _)) => img,
        Err(e) => handle_error(ctx, e),
    };

    // dockerfile is required for create-image
    let dockerfile_path = match image_config.dockerfile {
        Some(ref p) => p.clone(),
        None => handle_error(
            ctx,
            SodagunError {
                code: "CONFIG_INVALID",
                message:
                    "no 'dockerfile' in [image] — 'sandbox create-image' requires a dockerfile"
                        .to_string(),
            },
        ),
    };

    // Load and merge registry config
    let proj_registry = match load_registry_config(&config_path) {
        Ok(r) => r,
        Err(e) => handle_error(ctx, e),
    };
    let user_registry = match load_user_registry_config() {
        Ok(r) => r,
        Err(e) => handle_error(ctx, e),
    };
    let registry = merge_registry_configs(user_registry, proj_registry);

    // Load and merge user image overrides
    let user_image = match load_user_image_config() {
        Ok(u) => u,
        Err(e) => handle_error(ctx, e),
    };
    let image_config = merge_user_image_config(user_image, image_config);

    // Read Dockerfile bytes and compute the full OCI tag
    let dockerfile_bytes = std::fs::read(&dockerfile_path).unwrap_or_else(|e| {
        handle_error(
            ctx,
            SodagunError {
                code: "CONFIG_INVALID",
                message: format!(
                    "failed to read dockerfile '{}': {e}",
                    dockerfile_path.display()
                ),
            },
        )
    });
    let full_tag =
        match crate::config::dockerfile_image_tag(&image_config, &registry, &dockerfile_bytes) {
            Ok(t) => t,
            Err(e) => handle_error(ctx, e),
        };

    // Check whether the image already exists in the registry
    if !args.force && podman_manifest_exists(&full_tag) {
        match ctx.output {
            OutputFormat::Text => println!("Image already exists: {full_tag}"),
            OutputFormat::Json => println!(
                "{}",
                serde_json::json!({
                    "status": "ok",
                    "image_tag": full_tag,
                    "already_existed": true,
                })
            ),
        }
        return;
    }

    // Build
    let context_dir = config_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_str()
        .unwrap_or(".");
    let dockerfile_str = dockerfile_path.to_str().unwrap_or_else(|| {
        handle_error(
            ctx,
            SodagunError {
                code: "CONFIG_INVALID",
                message: "dockerfile path contains non-UTF-8 characters".to_string(),
            },
        )
    });

    if let Err(e) = run_podman(&["build", "-f", dockerfile_str, "-t", &full_tag, context_dir]) {
        handle_error(ctx, e);
    }

    // Push
    if let Err(e) = run_podman(&["push", &full_tag]) {
        handle_error(ctx, e);
    }

    match ctx.output {
        OutputFormat::Text => println!("Built and pushed: {full_tag}"),
        OutputFormat::Json => println!(
            "{}",
            serde_json::json!({
                "status": "ok",
                "image_tag": full_tag,
                "already_existed": false,
            })
        ),
    }
}

/// Returns true if `podman manifest inspect <tag>` exits 0 (image exists in registry).
fn podman_manifest_exists(tag: &str) -> bool {
    std::process::Command::new("podman")
        .args(["manifest", "inspect", tag])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a podman subcommand, streaming stdout/stderr to the terminal.
/// Returns `IMAGE_BUILD_ERROR` on non-zero exit.
fn run_podman(args: &[&str]) -> Result<(), SodagunError> {
    let status = std::process::Command::new("podman")
        .args(args)
        .status()
        .map_err(|e| SodagunError {
            code: "IMAGE_BUILD_ERROR",
            message: format!("failed to run podman: {e}"),
        })?;
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(SodagunError {
            code: "IMAGE_BUILD_ERROR",
            message: format!("podman {} exited with code {code}", args[0]),
        });
    }
    Ok(())
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
    if no_wait {
        // Fire-and-forget: spawn so we return before the SDK's internal stop wait.
        tokio::spawn(async move {
            let _ = handle.stop().await;
        });
    } else {
        handle
            .stop_with_timeout(timeout)
            .await
            .map_err(|e| SodagunError {
                code: "SANDBOX_ERROR",
                message: format!("failed to stop sandbox '{name}': {e}"),
            })?;
    }
    Ok(())
}

async fn remove_async(name: &str, timeout: Duration) -> Result<(), SodagunError> {
    let handle = Sandbox::get(name)
        .await
        .map_err(|e| util::map_sandbox_err(e, name))?;

    // Implicitly stop (with timeout) before removal; stop_with_timeout is a no-op
    // when the sandbox is already in a terminal state.
    handle
        .stop_with_timeout(timeout)
        .await
        .map_err(|e| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("failed to stop sandbox '{name}': {e}"),
        })?;

    Sandbox::remove(name)
        .await
        .map_err(|e| util::map_sandbox_err(e, name))
}

#[allow(clippy::too_many_arguments)]
async fn start_async(
    ctx: Context,
    sandbox_name: &str,
    worktree_path: &std::path::Path,
    image_config: &ImageConfig,
    sandbox_config: &SandboxConfig,
    registry: &RegistryConfig,
    // Pre-computed OCI tag when image_config.dockerfile is set.
    dockerfile_tag: Option<&str>,
    network_policies: &HashMap<String, NamedPolicy>,
    // Some(path) when the user's network-policies.toml file exists; None → use built-ins.
    policies_path: Option<&std::path::Path>,
) -> Result<String, SodagunError> {
    let mut builder = Sandbox::builder(sandbox_name);

    // Determine what to boot from.
    if let Some(tag) = dockerfile_tag {
        ctx.log(&format!("booting from OCI image: {tag}"));
        builder = builder.image(tag);
        if let Some(true) = registry.insecure {
            builder = builder.registry(|r| r.insecure());
        }
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
        let (host_path, guest_path, flags) = parse_volume(vol_str)?;
        let host_str = host_path
            .to_str()
            .ok_or_else(|| SodagunError {
                code: "CONFIG_INVALID",
                message: format!("volume host path is non-UTF-8: {}", host_path.display()),
            })?
            .to_owned();
        builder = builder.volume(guest_path, move |m| {
            let m = m.bind(&host_str);
            let m = if flags.readonly { m.readonly() } else { m };
            if flags.noexec { m.noexec() } else { m }
        });
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

    builder.create_detached().await.map_err(|e| {
        // When the image isn't found locally and a dockerfile tag was used,
        // give a hint to run create-image first.
        if matches!(e, MicrosandboxError::ImageNotFound(_)) && dockerfile_tag.is_some() {
            SodagunError {
                code: "SANDBOX_ERROR",
                message: format!("image not found — run 'sodagun sandbox create-image' first: {e}"),
            }
        } else {
            SodagunError {
                code: "SANDBOX_ERROR",
                message: format!("failed to create sandbox: {e}"),
            }
        }
    })?;

    Ok(sandbox_name.to_string())
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
