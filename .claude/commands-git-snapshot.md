# git & snapshot command reference

## `git add-worktree <branch-name> [repo-path]`

Creates a git worktree on a new branch inside a fresh workspace rootdir, writes `sodagun.json`, and prints the rootdir path to stdout.

Options:
- `repo-path` — positional; path to the git repo (default: auto-detected `project_dir`)
- `--base <ref>` — branch point (default: `origin/main`)
- `--dir-prefix <path>` — parent dir for the rootdir (default: system temp dir)

Rootdir: `<dir-prefix>/sodagun_<repo>_<branch>_<uuid8>` (note: `_`-delimited; `<repo>` and `<branch>` components are run through `util::dashify`, which collapses `/ _ : @` and space to `-`, keeping `_` unambiguous as the separator). The worktree itself lives at `<rootdir>/<sanitized-branch>`.

After creating the worktree, `add-worktree` normalizes the worktree's `commondir` (in `<repo>/.git/worktrees/<name>/commondir`) from the absolute host path libgit2 writes to the conventional relative `../..` (the form the git CLI uses), via `git_meta::normalize_commondir`. This is needed because some git codepaths read `commondir` even when `GIT_COMMON_DIR` is set, and an absolute host path there breaks guest git (mounted into a sandbox) with "Invalid path". Normalization failure (`GIT_ERROR`) rolls back the branch + rootdir.

JSON success: `{"status": "ok", "rootdir": "..."}`
JSON error: `{"status": "error", "code": "<CODE>"}`

Error codes: `REPO_NOT_FOUND`, `BASE_NOT_FOUND`, `BASE_INVALID`, `BRANCH_EXISTS`, `WORKTREE_EXISTS`, `GIT_ERROR`, `WORKSPACE_INVALID`

Error code mapping (git2):
- `repo_path.canonicalize()` fails → `REPO_NOT_FOUND` (resolved up front; no silent fallback)
- `Repository::open` fails → `REPO_NOT_FOUND`
- `revparse_single` fails with `ErrorCode::NotFound` → `BASE_NOT_FOUND`
- `revparse_single` fails with another error, or `peel_to_commit` fails → `BASE_INVALID`
- `repo.branch()` fails with `ErrorCode::Exists` → `BRANCH_EXISTS`
- rootdir/worktree path exists on disk OR name already in `repo.worktrees()` → `WORKTREE_EXISTS` (with branch rollback)
- `create_dir`/`repo.worktree()` fails for other reasons → `GIT_ERROR` (with branch + rootdir rollback)
- `WorkspaceMetadata::write` fails → `WORKSPACE_INVALID` (with full rollback)

## `snapshot create`

Builds a deterministically named snapshot by running the `[image]` setup script inside an ephemeral sandbox, then snapshotting it. Snapshot name: `<sanitized-base>_<12-char-sha256>`. `sandbox start` automatically boots from this snapshot when `[image]` configures a setup script; it errors with a hint if the snapshot hasn't been created yet.

Options:
- `--config <path>` — config file path (default: `<project-dir>/sodagun.toml`)
- `--force` — recreate even if the snapshot already exists

The ephemeral builder is sized by `snapshot_build_resources()`: half of total system RAM and all-but-two logical CPUs (minimum 1; unknown parallelism falls back to 1). It always runs with `NetworkPolicy::allow_all()` and an 8 GiB tmpfs `/tmp`.

Gotcha: the derived snapshot name hashes the setup script **and** the `setup_files` contents (for this repo, `Cargo.toml` / `Cargo.lock` / `rust-toolchain.toml`). Changing any of those — e.g. a dependency bump — changes the hash, so `sandbox start` will look for a snapshot that doesn't exist yet and you must re-run `sodagun snapshot create`.

JSON success: `{"status": "ok", "snapshot_name": "...", "already_existed": false}`

## `snapshot remove`

Resolves the derived snapshot name from the `[image]` config and removes it via `Snapshot::remove(name)`.

Options:
- `--config <path>` — config file path (default: `<project-dir>/sodagun.toml`)
- `-f` / `--force` — succeed silently if the snapshot does not exist

JSON success: `{"status": "ok"}`

## `snapshot clean`

Lists all snapshots, opens each to read labels, and removes those tagged `created_by=sodagun` AND `repo_path=<canonical project dir>`.

Options:
- `--config <path>` — config file path (default: `<project-dir>/sodagun.toml`)

JSON success: `{"status": "ok", "removed": ["..."]}`

## Snapshot error codes

`CONFIG_NOT_FOUND`, `CONFIG_INVALID`, `SNAPSHOT_NOT_FOUND`, `SNAPSHOT_ERROR`

- `SNAPSHOT_NOT_FOUND` — named snapshot does not exist (maps `MicrosandboxError::SnapshotNotFound`); emitted by `remove` (without `--force`), by `clean`, and by `sandbox start` when the derived snapshot is missing
- `SNAPSHOT_ERROR` — SDK failure during ephemeral sandbox creation, script execution, snapshotting, or remove
