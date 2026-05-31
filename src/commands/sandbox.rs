use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::config::{ImageConfig, NetworkMode, SandboxConfig};
use crate::context::{Context, OutputFormat};
use crate::error::{SodagunError, handle_error};
use crate::workspace::WorkspaceMetadata;
use clap::{Parser, Subcommand};
use microsandbox::sandbox::SandboxStatus;
use microsandbox::{MicrosandboxError, NetworkPolicy, Sandbox, Snapshot};

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

    /// Path to the sodagun config file (default: <worktree-path>/.sodagun.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(Parser)]
pub struct AttachArgs {
    /// Workspace rootdir of the sandbox to attach to.
    pub workspace_path: PathBuf,
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

fn make_runtime(ctx: Context) -> tokio::runtime::Runtime {
    match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => handle_error(
            ctx,
            SodagunError {
                code: "SANDBOX_ERROR",
                message: format!("failed to start async runtime: {e}"),
            },
        ),
    }
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

    let explicit_config = args.config.is_some();
    let config_path = args
        .config
        .unwrap_or_else(|| meta.worktree_path.join(".sodagun.toml"));

    let (image_config, sandbox_config) = if !config_path.exists() && !explicit_config {
        // No config file present; use conservative defaults (alpine:latest, airgapped, etc.)
        (
            crate::config::default_image_config(),
            crate::config::default_sandbox_config(),
        )
    } else {
        match crate::config::load_config(&config_path) {
            Ok(pair) => pair,
            Err(e) => handle_error(ctx, e),
        }
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

    let rt = make_runtime(ctx);
    let name = match rt.block_on(start_async(
        &sandbox_name,
        &meta.worktree_path,
        &image_config,
        &sandbox_config,
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

    let rt = make_runtime(ctx);
    match rt.block_on(attach_async(&sandbox_name)) {
        Ok(exit_code) => std::process::exit(exit_code),
        Err(e) => handle_error(ctx, e),
    }
}

fn exec(ctx: Context, args: ExecArgs) {
    let sandbox_name = read_sandbox_name(ctx, &args.workspace_path);

    let rt = make_runtime(ctx);
    match rt.block_on(exec_async(&sandbox_name, &args.cmd, &args.args)) {
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
    let rt = make_runtime(ctx);
    let sandboxes = match rt.block_on(list_async()) {
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
}

fn stop(ctx: Context, args: StopArgs) {
    let sandbox_name = read_sandbox_name(ctx, &args.workspace_path);

    let rt = make_runtime(ctx);
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

    let rt = make_runtime(ctx);
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

/// Parse a Docker-style volume string ("host:guest" or "host:guest:ro").
/// Expands a leading `~` in the host path to $HOME.
fn parse_volume(s: &str) -> Result<(PathBuf, String, bool), SodagunError> {
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    if parts.len() < 2 {
        return Err(SodagunError {
            code: "CONFIG_INVALID",
            message: format!("volume '{s}' must be 'host:guest' or 'host:guest:ro'"),
        });
    }
    let host_raw = parts[0];
    let guest = parts[1].to_string();
    let readonly = parts.get(2).is_some_and(|f| *f == "ro");

    let host = if let Some(rest) = host_raw.strip_prefix('~') {
        let home = std::env::var("HOME").map_err(|_| SodagunError {
            code: "CONFIG_INVALID",
            message: "cannot expand '~' in volume: $HOME is not set".to_string(),
        })?;
        PathBuf::from(format!("{home}{rest}"))
    } else {
        PathBuf::from(host_raw)
    };

    Ok((host, guest, readonly))
}

fn status_label(s: SandboxStatus) -> &'static str {
    match s {
        SandboxStatus::Running => "running",
        SandboxStatus::Draining => "draining",
        SandboxStatus::Paused => "paused",
        SandboxStatus::Stopped => "stopped",
        SandboxStatus::Crashed => "crashed",
    }
}

fn is_terminal_status(s: SandboxStatus) -> bool {
    matches!(s, SandboxStatus::Stopped | SandboxStatus::Crashed)
}

/// Maps a microsandbox SDK error to a SodagunError, using SANDBOX_NOT_FOUND for
/// unknown sandbox names and SANDBOX_ERROR for all other failures.
pub fn map_sandbox_sdk_err(e: MicrosandboxError, sandbox_name: &str) -> SodagunError {
    if matches!(e, MicrosandboxError::SandboxNotFound(_)) {
        SodagunError {
            code: "SANDBOX_NOT_FOUND",
            message: format!("sandbox '{sandbox_name}' not found"),
        }
    } else {
        SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("{e}"),
        }
    }
}

/// Polls `Sandbox::get` every 500ms until the sandbox reaches a terminal status
/// (Stopped or Crashed), or until `timeout` elapses. Checks status before sleeping
/// so fast-stopping sandboxes are detected immediately.
pub async fn poll_until_stopped(name: &str, timeout: Duration) -> Result<(), SodagunError> {
    let deadline = Instant::now() + timeout;
    loop {
        let handle = Sandbox::get(name)
            .await
            .map_err(|e| map_sandbox_sdk_err(e, name))?;
        if is_terminal_status(handle.status()) {
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

async fn list_async() -> Result<Vec<(String, String)>, SodagunError> {
    let handles = Sandbox::list().await.map_err(|e| SodagunError {
        code: "SANDBOX_ERROR",
        message: format!("failed to list sandboxes: {e}"),
    })?;
    Ok(handles
        .into_iter()
        .map(|h| (h.name().to_string(), status_label(h.status()).to_string()))
        .collect())
}

pub async fn stop_async(name: &str, timeout: Duration, no_wait: bool) -> Result<(), SodagunError> {
    let handle = Sandbox::get(name)
        .await
        .map_err(|e| map_sandbox_sdk_err(e, name))?;
    // Already terminal — stop is a no-op.
    if is_terminal_status(handle.status()) {
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

pub async fn remove_async(name: &str, timeout: Duration) -> Result<(), SodagunError> {
    let handle = Sandbox::get(name)
        .await
        .map_err(|e| map_sandbox_sdk_err(e, name))?;

    // Implicitly stop if still running before attempting removal.
    if !is_terminal_status(handle.status()) {
        handle.stop().await.map_err(|e| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("failed to send stop signal to '{name}': {e}"),
        })?;
        poll_until_stopped(name, timeout).await?;
    }

    Sandbox::remove(name)
        .await
        .map_err(|e| map_sandbox_sdk_err(e, name))
}

async fn start_async(
    sandbox_name: &str,
    worktree_path: &std::path::Path,
    image_config: &ImageConfig,
    sandbox_config: &SandboxConfig,
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
        builder = builder.from_snapshot(&snap_name);
    } else if let Some(ref image) = image_config.base_image {
        builder = builder.image(image.as_str());
    } else if let Some(ref snapshot) = image_config.base_snapshot {
        builder = builder.from_snapshot(snapshot.as_str());
    }

    builder = builder
        .cpus(sandbox_config.cpus)
        .memory(sandbox_config.memory_mb)
        .workdir(&sandbox_config.working_dir);

    builder = match sandbox_config.network.mode {
        NetworkMode::Airgapped => builder.disable_network(),
        NetworkMode::AllowAll => builder.network(|b| b.policy(NetworkPolicy::allow_all())),
        NetworkMode::PublicOnly => builder.network(|b| b.policy(NetworkPolicy::public_only())),
    };

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

    // Plain env vars
    builder = builder.envs(
        sandbox_config
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str())),
    );

    // Secrets — resolve value_from_env at launch time
    for (env_var, secret) in &sandbox_config.secrets {
        let value = if let Some(ref literal) = secret.value {
            literal.clone()
        } else if let Some(ref from_env) = secret.value_from_env {
            std::env::var(from_env).map_err(|_| SodagunError {
                code: "CONFIG_INVALID",
                message: format!(
                    "secret '{env_var}' references env var '{from_env}' which is not set"
                ),
            })?
        } else {
            return Err(SodagunError {
                code: "CONFIG_INVALID",
                message: format!("secret '{env_var}' has neither 'value' nor 'value_from_env'"),
            });
        };

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

async fn exec_async(
    sandbox_name: &str,
    cmd: &str,
    args: &[String],
) -> Result<microsandbox::sandbox::ExecOutput, SodagunError> {
    let sandbox = Sandbox::start(sandbox_name)
        .await
        .map_err(|e| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("failed to connect to sandbox '{sandbox_name}': {e}"),
        })?;

    sandbox
        .exec(cmd, args.iter().map(String::as_str))
        .await
        .map_err(|e| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("exec failed in sandbox '{sandbox_name}': {e}"),
        })
}

/// Returns the shell's exit code on a normal interactive session end.
/// Returns `Err` only on infrastructure failure (connection lost, etc.).
async fn attach_async(sandbox_name: &str) -> Result<i32, SodagunError> {
    let sandbox = Sandbox::start(sandbox_name)
        .await
        .map_err(|e| SodagunError {
            code: "SANDBOX_ERROR",
            message: format!("failed to connect to sandbox '{sandbox_name}': {e}"),
        })?;

    sandbox.attach_shell().await.map_err(|e| SodagunError {
        code: "SANDBOX_ERROR",
        message: format!("attach session failed: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    // Serialize tests that mutate $HOME to prevent races with parse_volume_tilde_expansion.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_volume_basic() {
        let (host, guest, ro) = parse_volume("/host/path:/guest/path").unwrap();
        assert_eq!(host, PathBuf::from("/host/path"));
        assert_eq!(guest, "/guest/path");
        assert!(!ro);
    }

    #[test]
    fn parse_volume_readonly() {
        let (_, _, ro) = parse_volume("/host:/guest:ro").unwrap();
        assert!(ro);
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
}
