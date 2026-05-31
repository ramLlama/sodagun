use std::path::PathBuf;

use crate::config;
use crate::context::{Context, OutputFormat};
use crate::error::{SodagunError, handle_error};
use clap::{Parser, Subcommand};
use microsandbox::{MicrosandboxError, NetworkPolicy, Sandbox, Snapshot};
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
    /// Name of the snapshot to remove.
    pub name: String,

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
    let rt = make_runtime(ctx);
    match rt.block_on(remove_async(&args.name, args.force)) {
        Ok(()) => match ctx.output {
            OutputFormat::Text => println!("Removed."),
            OutputFormat::Json => println!("{}", serde_json::json!({"status": "ok"})),
        },
        Err(e) => handle_error(ctx, e),
    }
}

async fn create_async(
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

    builder = builder
        .cpus(image_config.cpus)
        .memory(image_config.memory_mb)
        // Snapshot creation always uses full internet access for package installs.
        .network(|b| b.policy(NetworkPolicy::allow_all()))
        .script("setup", script);

    let sandbox = builder.create().await.map_err(|e| SodagunError {
        code: "SNAPSHOT_ERROR",
        message: format!("failed to create ephemeral sandbox: {e}"),
    })?;

    // Run the setup script and wait for completion.
    let output = sandbox.shell("setup").await.map_err(|e| SodagunError {
        code: "SNAPSHOT_ERROR",
        message: format!("setup script failed to execute: {e}"),
    })?;

    if !output.status().success {
        // Best-effort cleanup before returning error.
        let _ = Sandbox::remove(&ephemeral_name).await;
        return Err(SodagunError {
            code: "SNAPSHOT_ERROR",
            message: format!(
                "setup script exited with non-zero status (code {})",
                output.status().code
            ),
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
        eprintln!("warning: failed to remove ephemeral sandbox '{ephemeral_name}': {e}");
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
