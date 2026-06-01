# sodagun

Rust CLI tool built with clap. Primary use case: utilities for AI agents working in git repos.

## Commands

```
sodagun [--output text|json] <subcommand>
```

`--output` is parsed by the top-level `Cli` struct and must precede the subcommand (it is a top-level flag, not a true global). The selected `OutputFormat` is wrapped in a `Context` and passed by value into each handler.

### `git add-worktree <repo-path> <branch-name>`

Creates a git worktree on a new branch, prints the resulting path to stdout.

Options:
- `--base <ref>` — branch point (default: `origin/main`)
- `--dir-prefix <path>` — parent dir for the worktree (default: system temp dir)

Worktree path: `<dir-prefix>/sodagun-wt-<reponame>-<uuid8>`

JSON success: `{"status": "ok", "worktree_path": "..."}`
JSON error: `{"status": "error", "code": "<CODE>"}`

Error codes: `REPO_NOT_FOUND`, `BASE_NOT_FOUND`, `BASE_INVALID`, `BRANCH_EXISTS`, `WORKTREE_EXISTS`, `GIT_ERROR`

Error code mapping (git2):
- `Repository::open` fails → `REPO_NOT_FOUND`
- `revparse_single` fails with `ErrorCode::NotFound` → `BASE_NOT_FOUND`
- `revparse_single` fails with another error, or `peel_to_commit` fails → `BASE_INVALID`
- `repo.branch()` fails with `ErrorCode::Exists` → `BRANCH_EXISTS`
- worktree path exists on disk OR name already in `repo.worktrees()` → `WORKTREE_EXISTS` (with branch rollback)
- `repo.worktree()` fails for other reasons → `GIT_ERROR` (with branch rollback)

### `sandbox launch <worktree-path>`

Reads `sodagun.toml` from the worktree, creates a microsandbox via the microsandbox SDK, and prints the sandbox name. The worktree is bind-mounted at the configured `working_dir`.

Options:
- `--config <path>` — config file path (default: `<worktree-path>/sodagun.toml`)

Sandbox name: `sodagun-sb-<worktree-dirname>-<uuid8>`

JSON success: `{"status": "ok", "sandbox_name": "..."}`
JSON error: `{"status": "error", "code": "<CODE>"}`

### `sandbox attach <sandbox-name>`

Reconnects to a running named sandbox (`Sandbox::start`) and attaches an interactive TTY shell (`attach_shell()`). On a normal session end, exits with the shell's exit code via `std::process::exit()`. Only emits a `SANDBOX_ERROR` on infrastructure failure (connection lost, etc.).

### `sandbox list`

Lists all sandboxes and their statuses via `Sandbox::list()`.

Text success: aligned `NAME` / `STATUS` table (status lowercased).
JSON success: `{"status": "ok", "sandboxes": [{"name": "...", "status": "running"}, ...]}`

### `sandbox stop <sandbox-name>`

Sends a graceful shutdown signal to a named sandbox via `Sandbox::get(name)` → `handle.stop()`.

Options:
- `--stop-timeout-seconds <N>` — seconds to poll for the sandbox to reach `stopped`/`crashed` (default: 30)
- `--no-wait` — return immediately after sending the stop signal without polling

Text success: `"Stopped."` (or `"Stop signal sent."` with `--no-wait`)
JSON success: `{"status": "ok"}`

### `sandbox remove <sandbox-name>`

Removes a sandbox. If it is still running, sends a stop signal and polls until it halts before calling `Sandbox::remove(name)`.

Options:
- `--stop-timeout-seconds <N>` — seconds to wait for the implicit stop phase (default: 30)

Text success: `"Removed."`
JSON success: `{"status": "ok"}`

### `snapshot create <rootdir>`

Builds a deterministically named snapshot by running the `[image]` setup script inside an ephemeral sandbox, then snapshotting it. Snapshot name: `<sanitized-base>_<12-char-sha256>`. `sandbox launch` automatically boots from this snapshot when `[image]` is configured; it errors with a hint if the snapshot hasn't been created yet.

Options:
- `--config <path>` — config file path (default: `<rootdir>/sodagun.toml`)
- `--force` — recreate even if the snapshot already exists

JSON success: `{"status": "ok", "snapshot_name": "...", "already_existed": false}`

### `snapshot remove <name>`

Removes a named snapshot via `Snapshot::remove(name)`.

Options:
- `-f` / `--force` — succeed silently if the snapshot does not exist

JSON success: `{"status": "ok"}`

### Snapshot error codes

`CONFIG_NOT_FOUND`, `CONFIG_INVALID`, `SNAPSHOT_NOT_FOUND`, `SNAPSHOT_ERROR`

- `SNAPSHOT_NOT_FOUND` — named snapshot does not exist; emitted by `remove` (without `--force`)
- `SNAPSHOT_ERROR` — SDK failure during ephemeral sandbox creation, script execution, snapshotting, or remove

### Sandbox error codes

`WORKTREE_NOT_FOUND`, `CONFIG_NOT_FOUND`, `CONFIG_INVALID`, `SANDBOX_NOT_FOUND`, `SANDBOX_ERROR`

- `WORKTREE_NOT_FOUND` — worktree path does not exist or is not a directory
- `CONFIG_NOT_FOUND` — `sodagun.toml` missing from the config path
- `CONFIG_INVALID` — malformed TOML; missing `base_image`/`base_snapshot` in `[image]`; both set together; `setup_script`+`setup_script_path` conflict; a missing/unreadable `setup_files` entry; a `setup_files` entry with a non-UTF-8 basename; env/secret key conflict; invalid network mode; `cpus` out of `u8` range; bad volume format; `$HOME` not set for `~` expansion; unresolvable `value_from_env`; non-UTF-8 paths
- `SANDBOX_NOT_FOUND` — named sandbox does not exist (maps `MicrosandboxError::SandboxNotFound`); emitted by `stop`, `remove`
- `SANDBOX_ERROR` — microsandbox SDK failure (runtime creation, `create_detached`, `start`, `attach_shell`, stop/remove ops, stop timeout)

### `sodagun.toml` format

```toml
[image]
base_image = "debian"         # or base_snapshot = "name" (mutually exclusive; exactly one required)
memory_mb = 512               # sandbox used during snapshot create; default
cpus = 1                      # default; type u8
setup_script = """
#!/usr/bin/env bash
set -e
apt-get install -y git
"""
# setup_script_path = "./setup.sh"  # alternative to inline setup_script (mutually exclusive)
setup_files = ["rust-toolchain.toml", "Cargo.toml"]  # paths relative to config file; injected into /setup-assets/ during snapshot create

[sandbox]
working_dir = "/workspace"  # default
memory_mb = 512             # default; type u32
cpus = 1                    # default; type u8 (serde rejects values > 255 at parse time)
volumes = ["~/.config/claude:/root/.config/claude:ro"]

[sandbox.network]
mode = "airgapped"   # default; options: airgapped, public-only, allow-all (kebab-case). This repo's own sodagun.toml uses airgapped (deps are pre-fetched into the snapshot via setup_files + cargo fetch)

[sandbox.env]
TERM = "xterm-256color"

[sandbox.secrets.ANTHROPIC_API_KEY]
value_from_env = "ANTHROPIC_API_KEY"  # or: value = "literal"
allowed_hosts = ["api.anthropic.com"]
```

`[image]` key invariants:
- Exactly one of `base_image` / `base_snapshot` is required; they are mutually exclusive
- At most one of `setup_script` / `setup_script_path`; they are mutually exclusive
- `setup_script_path` is resolved relative to the config file at load time
- `setup_files` is a list of paths relative to the config file; each is resolved at load time into a `SetupFile { name, content }` (basename + raw bytes) and injected into `/setup-assets/<name>` via a patch during snapshot creation. A missing/unreadable entry or a non-UTF-8 basename is `CONFIG_INVALID`
- Snapshot name is `<sanitized-base>_<first-12-base64url-chars-of-sha256(script + setup_files)>`; setup file contents are hashed sorted by name, so the name is deterministic given the same base + script + setup_files

Sandbox key invariants:
- `[image]` section is required; `[sandbox]` is optional (all fields have defaults)
- A key may not appear in both `[sandbox.env]` and `[sandbox.secrets]` (validated in `load_config`)
- Network mode maps to the SDK: `airgapped` → `disable_network()`, `allow-all` → `NetworkPolicy::allow_all()`, `public-only` → `NetworkPolicy::public_only()`
- `cpus` is `u8` so serde rejects out-of-range values at parse time with `CONFIG_INVALID`
- Volume strings are Docker-style `"host:guest"` or `"host:guest:ro"`; tilde (`~`) expansion to `$HOME` happens at launch time (`parse_volume`), not config-parse time
- `value_from_env` in secrets is resolved at launch time, not config-parse time, so secret values stay out of the parsed struct
- Secret `allowed_hosts` entries containing `*` use `allow_host_pattern`; others use `allow_host`
- Async work runs on a fresh `tokio` multi-thread runtime per invocation, created in `make_runtime`; failure to build it is a `SANDBOX_ERROR`

## Architecture

```
src/
  main.rs             # clap Cli struct, main(), dispatch
  context.rs          # OutputFormat (clap::ValueEnum, Default) + Context struct
  error.rs            # SodagunError (now #[derive(Debug)]), handle_error() -> !
  config.rs           # sodagun.toml parser; ImageConfig (incl. setup_files: Vec<SetupFile>), SetupFile { name, content }, SandboxConfig, NetworkConfig, SecretConfig, NetworkMode, load_config(), load_image_config(), snapshot_name()
  commands/
    mod.rs
    git.rs            # GitCommand sub-app; add_worktree logic
    sandbox.rs        # SandboxCommand sub-app; launch()/attach()/list()/stop()/remove() + async impls, parse_volume(), poll_until_stopped(), map_sdk_err(), status_label()
    snapshot.rs       # SnapshotCommand sub-app; create()/remove() + async impls, derived_snapshot_name()
tests/
  integration/
    test_add_worktree.rs       # registered via [[test]] in Cargo.toml
    test_sandbox_start.rs      # registered via [[test]] in Cargo.toml
    test_sandbox_lifecycle.rs  # registered via [[test]] in Cargo.toml
    test_snapshot.rs           # registered via [[test]] in Cargo.toml
Cargo.toml
deny.toml             # cargo-deny policy (permissive license allowances + microsandbox advisory ignores)
Makefile
.pre-commit-config.yaml
```

Key invariants:
- `--output` must precede the subcommand (not a true global); `Context` is constructed in `main()` and passed by value into each handler
- `handle_error()` returns `!` (Never type) -- always calls `std::process::exit(1)` after printing; this is the Rust equivalent of Python's `NoReturn`
- Text errors go to stderr (`eprintln!`); JSON errors go to stdout (`println!`) so `--output json` output is always parseable
- `git2::Repository::worktree()` requires the target path to not pre-exist
- Branch is rolled back (`branch.delete()`) manually if worktree creation fails -- no RAII guard yet
- `repo.revparse_single()` returns `ErrorCode::NotFound` for unknown refs (equivalent to Python's `KeyError`) -- caught separately from other git errors
- Top-level error handling uses the `handle_error(ctx, SodagunError { code, message }) -> !` pattern rather than `?` / `Result` propagation, so error codes and exit semantics stay explicit
- `sandbox attach` is the one command that exits with a non-1 code on success: it propagates the shell's exit code via `std::process::exit()` rather than printing a success payload
- Async sandbox SDK calls are bridged to the synchronous handlers with a per-invocation `tokio` multi-thread runtime (`make_runtime`); the `launch_async` / `attach_async` / `list_async` / `stop_async` / `remove_async` functions own all `.await`s
- `map_sdk_err()` in `sandbox.rs` maps `MicrosandboxError::SandboxNotFound` → `SANDBOX_NOT_FOUND` and all other SDK errors → `SANDBOX_ERROR`
- `poll_until_stopped()` polls `Sandbox::get().status()` every 500ms using `tokio::time::sleep`; returns `SANDBOX_ERROR` on timeout
- `snapshot create` builds the ephemeral sandbox with `.patch(...)` (replacing the older `.script()` + `shell_stream()` approach): the setup script is patched to `/setup-assets/setup` with mode `0o755`, and each `setup_files` entry is patched to `/setup-assets/<name>`. The script is then run directly via `sandbox.exec_stream("/setup-assets/setup", ...)`. The resulting snapshot carries a `setup_hash` label equal to the 12-char base64url suffix of the snapshot name (which covers both the script and setup_files)

## Dev workflow

```bash
make all        # fmt + lint + typecheck + test + audit
make test       # cargo test
make typecheck  # cargo check
make lint       # cargo clippy --all-targets --all-features -- -D warnings
make fmt        # cargo fmt --all
make audit      # cargo deny check && cargo audit
make build      # cargo build --release
make install    # cargo install --path .
```

## Testing conventions

- `tests/integration/test_add_worktree.rs` -- end-to-end tests that invoke the compiled binary via `assert_cmd::Command::cargo_bin("sodagun")`; registered via `[[test]]` in `Cargo.toml` so the `tests/integration/` layout works (Cargo's default discovery only picks up files directly under `tests/`)
- Integration helper `make_git_repo()` does `init_repository`, creates one commit, and writes a `refs/remotes/origin/main` ref so `--base origin/main` resolves out of the box
- Unit tests live in `#[cfg(test)]` modules inside `src/commands/git.rs` and cover the pure naming contract (e.g. worktree path construction); no mocking layer for `git2`
- `tests/integration/test_sandbox_start.rs` -- error-path tests (CONFIG_NOT_FOUND, malformed TOML, `[image]` validation errors, each in text + json) plus `launch_creates_sandbox`, which is `#[ignore]`d (reason: "requires KVM or Apple Silicon hvf, and a valid image") since it needs hardware virtualization
- `tests/integration/test_sandbox_lifecycle.rs` -- list (JSON shape + text header), stop/remove SANDBOX_NOT_FOUND (text + json each), plus two `#[ignore]`d happy-path tests (stop running sandbox, remove running sandbox with implicit stop)
- `tests/integration/test_snapshot.rs` -- all `[image]` error paths (CONFIG_NOT_FOUND, malformed TOML, missing base, conflicting fields), plus `#[ignore]`d e2e happy-path test that installs git via setup script and asserts `git version` succeeds in a sandbox booted from the resulting snapshot
- `src/config.rs` has `#[cfg(test)]` unit tests covering valid `[image]` configs, defaults, and every `CONFIG_INVALID` / `CONFIG_NOT_FOUND` path; `parse_volume` unit tests live in `src/commands/sandbox.rs`
- Volume tilde tests in `src/config.rs` assert `~` is preserved (not expanded) at parse time; `sandbox.rs` tests cover the launch-time expansion (and the `$HOME`-unset error path, which mutates the env var)

## Dependencies

Runtime: `clap` (derive), `git2` (`vendored-libgit2`), `microsandbox` (0.4), `serde` + `serde_json`, `toml`, `tokio` (`rt-multi-thread`, `time`), `uuid` (v4), `colored`, `sha2`, `base64`, `hex`
Dev: `assert_cmd`, `predicates`, `tempfile`
Supply chain: `cargo-deny` + `cargo-audit` wired into pre-commit and `make audit`; `Cargo.lock` is committed. `deny.toml` allows additional permissive licenses (ISC, BSD-3-Clause, 0BSD, CDLA-Permissive-2.0, etc.) and ignores specific advisories pulled in by `microsandbox` transitive deps; `make audit` mirrors those ignores with `--ignore` flags

## Style

- Rust 2024 edition, no `unsafe`
- Error handling at the top level uses `handle_error(ctx, SodagunError { code, message }) -> !` (not `?` propagation) so every exit point carries an explicit error code
- `colored` crate for styled stderr: `"Error".red().bold()`
- Non-trivial functions get docstrings (`///`); comment blocks that do non-obvious work (e.g. why `file_name()` is used instead of `canonicalize()`)