use std::path::PathBuf;

use crate::config;
use crate::context::{Context, OutputFormat};
use crate::error::{SodagunError, handle_error};
use clap::{Parser, Subcommand};
use microsandbox::{ExecEvent, MicrosandboxError, NetworkPolicy, Sandbox, Snapshot};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Parser)]
pub struct SnapshotCommand {
    #[command(subcommand)]
    pub subcommand: SnapshotSubcommand,
}

#[derive(Subcommand)]
pub enum SnapshotSubcommand {
    /// Create a snapshot from the [image] setup_script in .sodagun.toml.
    Create(CreateArgs),
    /// Remove a named snapshot.
    Remove(RemoveArgs),
}

#[derive(Parser)]
pub struct CreateArgs {
    /// Workspace rootdir containing .sodagun.toml.
    pub rootdir: PathBuf,

    /// Path to the config file (default: <rootdir>/.sodagun.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Force recreation even if the snapshot already exists.
    #[arg(long)]
    pub force: bool,
}

#[derive(Parser)]
pub struct RemoveArgs {
    /// Workspace rootdir containing .sodagun.toml.
    pub rootdir: PathBuf,

    /// Path to the config file (default: <rootdir>/.sodagun.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Succeed silently even if the snapshot does not exist (like rm -f).
    #[arg(long, short = 'f')]
    pub force: bool,
}

pub fn run(ctx: Context, cmd: SnapshotCommand) {
    match cmd.subcommand {
        SnapshotSubcommand::Create(args) => create(ctx, args),
        SnapshotSubcommand::Remove(args) => remove(ctx, args),
    }
}

fn make_runtime(ctx: Context) -> tokio::runtime::Runtime {
    match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => handle_error(
            ctx,
            SodagunError {
                code: "SNAPSHOT_ERROR",
                message: format!("failed to start async runtime: {e}"),
            },
        ),
    }
}

/// Outcome of a `snapshot create` call.
enum CreateOutcome {
    /// Snapshot already existed and `--force` was not set.
    AlreadyExists,
    /// A new snapshot was successfully created.
    Created,
}

fn create(ctx: Context, args: CreateArgs) {
    let config_path = args
        .config
        .unwrap_or_else(|| args.rootdir.join(".sodagun.toml"));

    let image_config = match config::load_image_config(&config_path) {
        Ok(c) => c,
        Err(e) => handle_error(ctx, e),
    };

    let script = match image_config.setup_script {
        Some(ref s) => s.clone(),
        None => handle_error(
            ctx,
            SodagunError {
                code: "CONFIG_INVALID",
                message: "no setup_script or setup_script_path in [image] — nothing to snapshot"
                    .to_string(),
            },
        ),
    };

    let snapshot_name = image_config
        .derived_snapshot_name()
        .expect("derived_snapshot_name is Some when setup_script is Some");

    let rt = make_runtime(ctx);
    let outcome = match rt.block_on(create_async(
        ctx,
        &image_config,
        &script,
        &snapshot_name,
        args.force,
    )) {
        Ok(o) => o,
        Err(e) => handle_error(ctx, e),
    };

    match outcome {
        CreateOutcome::AlreadyExists => match ctx.output {
            OutputFormat::Text => println!("Snapshot already exists: {snapshot_name}"),
            OutputFormat::Json => println!(
                "{}",
                serde_json::json!({
                    "status": "ok",
                    "snapshot_name": snapshot_name,
                    "already_existed": true,
                })
            ),
        },
        CreateOutcome::Created => match ctx.output {
            OutputFormat::Text => println!("Created snapshot: {snapshot_name}"),
            OutputFormat::Json => println!(
                "{}",
                serde_json::json!({
                    "status": "ok",
                    "snapshot_name": snapshot_name,
                    "already_existed": false,
                })
            ),
        },
    }
}

fn remove(ctx: Context, args: RemoveArgs) {
    let config_path = args
        .config
        .unwrap_or_else(|| args.rootdir.join(".sodagun.toml"));

    let image_config = match config::load_image_config(&config_path) {
        Ok(c) => c,
        Err(e) => handle_error(ctx, e),
    };

    let snapshot_name = match image_config.derived_snapshot_name() {
        Some(n) => n,
        None => handle_error(
            ctx,
            SodagunError {
                code: "CONFIG_INVALID",
                message: "no setup_script or setup_script_path in [image] — nothing to remove"
                    .to_string(),
            },
        ),
    };

    let rt = make_runtime(ctx);
    match rt.block_on(remove_async(&snapshot_name, args.force)) {
        Ok(()) => match ctx.output {
            OutputFormat::Text => println!("Removed."),
            OutputFormat::Json => println!("{}", serde_json::json!({"status": "ok"})),
        },
        Err(e) => handle_error(ctx, e),
    }
}

/// Returns (memory_mb, cpus) sized for a fast snapshot build:
/// half of total system RAM and all-but-two logical CPUs (minimum 1).
fn snapshot_build_resources() -> (u32, u8) {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    let memory_mb = (sys.total_memory() / 1_048_576 / 2) as u32;
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    let cpus = cpus.saturating_sub(2).max(1) as u8;
    (memory_mb, cpus)
}

async fn create_async(
    ctx: Context,
    image_config: &config::ImageConfig,
    script: &str,
    snapshot_name: &str,
    force: bool,
) -> Result<CreateOutcome, SodagunError> {
    // Check whether the snapshot already exists.
    let exists = match Snapshot::get(snapshot_name).await {
        Ok(_) => true,
        Err(MicrosandboxError::SnapshotNotFound(_)) => false,
        Err(e) => {
            return Err(SodagunError {
                code: "SNAPSHOT_ERROR",
                message: format!("failed to check for existing snapshot: {e}"),
            });
        }
    };

    if exists && !force {
        return Ok(CreateOutcome::AlreadyExists);
    }

    if exists {
        Snapshot::remove(snapshot_name, true)
            .await
            .map_err(|e| SodagunError {
                code: "SNAPSHOT_ERROR",
                message: format!("failed to remove existing snapshot '{snapshot_name}': {e}"),
            })?;
    }

    // Ephemeral sandbox used only to run the setup script, then snapshot.
    let ephemeral_name = format!("sodagun-snap-{}", &Uuid::new_v4().to_string()[..8]);

    let mut builder = Sandbox::builder(&ephemeral_name);

    if let Some(ref image) = image_config.base_image {
        builder = builder.image(image.as_str());
    } else if let Some(ref snap) = image_config.base_snapshot {
        builder = builder.from_snapshot(snap.as_str());
    }

    let (snap_memory_mb, snap_cpus) = snapshot_build_resources();
    ctx.log(&format!(
        "snapshot build resources: {snap_memory_mb} MiB RAM, {snap_cpus} CPUs"
    ));
    builder = builder
        .cpus(snap_cpus)
        .memory(snap_memory_mb)
        // Snapshot creation always uses full internet access for package installs.
        .network(|b| b.policy(NetworkPolicy::allow_all()))
        // Give /tmp plenty of room for build artifacts (e.g. cargo compiling large C deps).
        .volume("/tmp", |m| m.tmpfs().size(8192u32))
        .script("setup", script);

    let sandbox = builder.create().await.map_err(|e| SodagunError {
        code: "SNAPSHOT_ERROR",
        message: format!("failed to create ephemeral sandbox: {e}"),
    })?;

    // Run the setup script, streaming stdout/stderr to the user unless --quiet.
    let mut handle = sandbox
        .shell_stream("setup")
        .await
        .map_err(|e| SodagunError {
            code: "SNAPSHOT_ERROR",
            message: format!("setup script failed to execute: {e}"),
        })?;

    // exit_code: None until Exited fires; Some(code) on completion.
    let exit_code: i32 = loop {
        match handle.recv().await {
            Some(ExecEvent::Stdout(data)) => {
                ctx.log(String::from_utf8_lossy(&data).trim_end_matches('\n'));
            }
            Some(ExecEvent::Stderr(data)) => {
                ctx.log(String::from_utf8_lossy(&data).trim_end_matches('\n'));
            }
            Some(ExecEvent::Exited { code }) => break code,
            Some(ExecEvent::Failed(payload)) => {
                let _ = Sandbox::remove(&ephemeral_name).await;
                return Err(SodagunError {
                    code: "SNAPSHOT_ERROR",
                    message: format!("setup script failed to spawn: {payload:?}"),
                });
            }
            Some(_) => {}
            None => {
                let _ = Sandbox::remove(&ephemeral_name).await;
                return Err(SodagunError {
                    code: "SNAPSHOT_ERROR",
                    message: "setup script exec session ended without exit event".to_string(),
                });
            }
        }
    };

    if exit_code != 0 {
        // Best-effort cleanup before returning error.
        let _ = Sandbox::remove(&ephemeral_name).await;
        return Err(SodagunError {
            code: "SNAPSHOT_ERROR",
            message: format!("setup script exited with non-zero status (code {exit_code})"),
        });
    }

    // Snapshots require a stopped sandbox.
    sandbox.stop_and_wait().await.map_err(|e| SodagunError {
        code: "SNAPSHOT_ERROR",
        message: format!("failed to stop ephemeral sandbox: {e}"),
    })?;

    let hex_sha256 = hex::encode(Sha256::digest(script.as_bytes()));
    let source_ref = image_config
        .base_image
        .as_deref()
        .or(image_config.base_snapshot.as_deref())
        .unwrap_or("");

    Snapshot::builder(&ephemeral_name)
        .name(snapshot_name)
        .label("setup_script_sha256", &hex_sha256)
        .label("source_image", source_ref)
        .label("created_by", "sodagun")
        .create()
        .await
        .map_err(|e| SodagunError {
            code: "SNAPSHOT_ERROR",
            message: format!("failed to create snapshot: {e}"),
        })?;

    // Best-effort removal of the ephemeral sandbox; warn but do not fail the command.
    if let Err(e) = Sandbox::remove(&ephemeral_name).await {
        ctx.warn(&format!(
            "failed to remove ephemeral sandbox '{ephemeral_name}': {e}"
        ));
    }

    Ok(CreateOutcome::Created)
}

async fn remove_async(name: &str, force: bool) -> Result<(), SodagunError> {
    match Snapshot::remove(name, false).await {
        Ok(()) => Ok(()),
        Err(MicrosandboxError::SnapshotNotFound(_)) => {
            if force {
                Ok(())
            } else {
                Err(SodagunError {
                    code: "SNAPSHOT_NOT_FOUND",
                    message: format!("snapshot '{name}' not found"),
                })
            }
        }
        Err(e) => Err(SodagunError {
            code: "SNAPSHOT_ERROR",
            message: format!("failed to remove snapshot '{name}': {e}"),
        }),
    }
}
