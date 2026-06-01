use std::path::PathBuf;

use crate::config;
use crate::context::{Context, OutputFormat};
use crate::error::{SodagunError, handle_error};
use crate::util;
use clap::{Parser, Subcommand};
use microsandbox::{ExecEvent, MicrosandboxError, NetworkPolicy, Sandbox, Snapshot};
use uuid::Uuid;

/// Guest directory the setup script and `setup_files` are patched into during
/// snapshot creation. The script lands at `<SETUP_ASSETS_DIR>/<SETUP_SCRIPT_NAME>`.
const SETUP_ASSETS_DIR: &str = "/setup-assets";

#[derive(Parser)]
pub struct SnapshotCommand {
    #[command(subcommand)]
    pub subcommand: SnapshotSubcommand,
}

#[derive(Subcommand)]
pub enum SnapshotSubcommand {
    /// Create a snapshot from the [image] setup_script in sodagun.toml.
    Create(CreateArgs),
    /// Remove a named snapshot.
    Remove(RemoveArgs),
    /// Remove all snapshots associated with the current repo.
    Clean(CleanArgs),
}

#[derive(Parser)]
pub struct CreateArgs {
    /// Path to the config file (default: <project-dir>/sodagun.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Force recreation even if the snapshot already exists.
    #[arg(long)]
    pub force: bool,
}

#[derive(Parser)]
pub struct RemoveArgs {
    /// Path to the config file (default: <project-dir>/sodagun.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Succeed silently even if the snapshot does not exist (like rm -f).
    #[arg(long, short = 'f')]
    pub force: bool,
}

#[derive(Parser)]
pub struct CleanArgs {
    /// Path to the config file (default: <project-dir>/sodagun.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,
}

pub fn run(ctx: Context, cmd: SnapshotCommand, project_dir: PathBuf) {
    match cmd.subcommand {
        SnapshotSubcommand::Create(args) => create(ctx, args, project_dir),
        SnapshotSubcommand::Remove(args) => remove(ctx, args, project_dir),
        SnapshotSubcommand::Clean(args) => clean(ctx, args, project_dir),
    }
}

/// Outcome of a `snapshot create` call.
enum CreateOutcome {
    /// Snapshot already existed and `--force` was not set.
    AlreadyExists,
    /// A new snapshot was successfully created.
    Created,
}

fn create(ctx: Context, args: CreateArgs, project_dir: PathBuf) {
    let config_path = args
        .config
        .unwrap_or_else(|| project_dir.join("sodagun.toml"));

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

    // Canonical repo path stored as a label so `snapshot clean` can filter by repo.
    let repo_path = config_path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    let rt = util::get_runtime();
    let outcome = match rt.block_on(create_async(
        ctx,
        &image_config,
        &script,
        &snapshot_name,
        &repo_path,
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

fn remove(ctx: Context, args: RemoveArgs, project_dir: PathBuf) {
    let config_path = args
        .config
        .unwrap_or_else(|| project_dir.join("sodagun.toml"));

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

    let rt = util::get_runtime();
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
    // If parallelism is unknown, assume a single core (the saturating_sub below
    // then floors at 1 either way).
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let cpus = cpus.saturating_sub(2).max(1) as u8;
    (memory_mb, cpus)
}

async fn create_async(
    ctx: Context,
    image_config: &config::ImageConfig,
    script: &str,
    snapshot_name: &str,
    repo_path: &str,
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
    let script_path = format!("{SETUP_ASSETS_DIR}/{}", config::SETUP_SCRIPT_NAME);
    builder = builder
        .cpus(snap_cpus)
        .memory(snap_memory_mb)
        // Snapshot creation always uses full internet access for package installs.
        .network(|b| b.policy(NetworkPolicy::allow_all()))
        // Give /tmp plenty of room for build artifacts (e.g. cargo compiling large C deps).
        .volume("/tmp", |m| m.tmpfs().size(8192u32))
        // Env vars for the ephemeral build sandbox (e.g. HOME).
        .envs(
            image_config
                .env
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        )
        // Inject the setup script and any setup_files into SETUP_ASSETS_DIR via patches.
        .patch(|p| {
            let mut p = p.text(script_path.as_str(), script, Some(0o755), false);
            for f in &image_config.setup_files {
                p = p.file(
                    format!("{SETUP_ASSETS_DIR}/{}", f.name),
                    f.content.clone(),
                    None,
                    false,
                );
            }
            p
        });

    let sandbox = builder.create().await.map_err(|e| SodagunError {
        code: "SNAPSHOT_ERROR",
        message: format!("failed to create ephemeral sandbox: {e}"),
    })?;

    // Run the setup script directly (no shell wrapper — script has a shebang and is mode 0o755).
    let mut handle = sandbox
        .exec_stream(script_path.as_str(), std::iter::empty::<&str>())
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

    // Flush all pending guest writes so small files (page-cache dirty) aren't
    // lost when the VM halts before the upper layer is snapshotted.
    if let Err(e) = sandbox.exec("sync", std::iter::empty::<&str>()).await {
        ctx.warn(&format!("sync before snapshot failed (continuing): {e}"));
    }

    // Snapshots require a stopped sandbox.
    sandbox.stop_and_wait().await.map_err(|e| SodagunError {
        code: "SNAPSHOT_ERROR",
        message: format!("failed to stop ephemeral sandbox: {e}"),
    })?;

    // The 12-char base64url hash suffix from the snapshot name is already the authoritative
    // combined hash of the script + setup_files, so reuse it as the label value.
    let setup_hash = snapshot_name.rsplit_once('_').map(|(_, h)| h).unwrap_or("");
    let source_ref = image_config
        .base_image
        .as_deref()
        .or(image_config.base_snapshot.as_deref())
        .unwrap_or("");

    Snapshot::builder(&ephemeral_name)
        .name(snapshot_name)
        .label("created_by", "sodagun")
        .label("repo_path", repo_path)
        .label("setup_hash", setup_hash)
        .label("source_image", source_ref)
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

fn clean(ctx: Context, args: CleanArgs, project_dir: PathBuf) {
    let config_path = args
        .config
        .unwrap_or_else(|| project_dir.join("sodagun.toml"));

    let repo_path = match config_path.parent().and_then(|p| p.canonicalize().ok()) {
        Some(p) => p.to_string_lossy().into_owned(),
        None => handle_error(
            ctx,
            SodagunError {
                code: "CONFIG_INVALID",
                message: "cannot canonicalize project directory".to_string(),
            },
        ),
    };

    let rt = util::get_runtime();
    let removed = match rt.block_on(clean_async(ctx, &repo_path)) {
        Ok(r) => r,
        Err(e) => handle_error(ctx, e),
    };

    match ctx.output {
        OutputFormat::Text => {
            if removed.is_empty() {
                println!("No snapshots to clean.");
            } else {
                for name in &removed {
                    println!("Removed: {name}");
                }
            }
        }
        OutputFormat::Json => println!(
            "{}",
            serde_json::json!({"status": "ok", "removed": removed})
        ),
    }
}

/// Lists all snapshots, opens each to read labels, removes those tagged with `repo_path`.
async fn clean_async(ctx: Context, repo_path: &str) -> Result<Vec<String>, SodagunError> {
    let handles = Snapshot::list().await.map_err(|e| SodagunError {
        code: "SNAPSHOT_ERROR",
        message: format!("failed to list snapshots: {e}"),
    })?;

    let mut removed = Vec::new();
    for handle in handles {
        let snapshot = match handle.open().await {
            Ok(s) => s,
            // Skip snapshots whose artifact is no longer on disk.
            Err(_) => continue,
        };

        let labels = &snapshot.manifest().labels;
        if labels.get("created_by").map(String::as_str) != Some("sodagun")
            || labels.get("repo_path").map(String::as_str) != Some(repo_path)
        {
            continue;
        }

        // Prefer the name alias for removal; fall back to the content digest.
        let id = handle
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| handle.digest().to_owned());

        ctx.log(&format!("removing snapshot: {id}"));
        Snapshot::remove(&id, false)
            .await
            .map_err(|e| util::map_snapshot_err(e, &id))?;

        removed.push(id);
    }

    Ok(removed)
}

async fn remove_async(name: &str, force: bool) -> Result<(), SodagunError> {
    match Snapshot::remove(name, false).await {
        Ok(()) => Ok(()),
        // `rm -f` semantics: a missing snapshot is success.
        Err(MicrosandboxError::SnapshotNotFound(_)) if force => Ok(()),
        Err(e) => Err(util::map_snapshot_err(e, name)),
    }
}
