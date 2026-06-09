# Architecture (detailed invariants)

The compact `src/` tree and the most important invariants live in [CLAUDE.md](CLAUDE.md). This file holds the remaining key invariants.

## Key invariants

- Top-level flags (`--output`/`--quiet`/`--project-dir`) must precede the subcommand (not true globals); `Context { output, quiet }` is constructed in `main()` and passed by value into each handler. `project_dir` is resolved by `find_project_dir()` and passed to the `git`/`snapshot` handlers
- `git2::Repository::worktree()` requires the target path to not pre-exist; the worktree name stored under `.git/worktrees/` is the dashified branch (git can't nest dirs there)
- Branch / rootdir / metadata are rolled back manually on any post-creation failure (no RAII guard yet)
- `repo.revparse_single()` returns `ErrorCode::NotFound` for unknown refs (equivalent to Python's `KeyError`) — caught separately from other git errors
- Top-level error handling uses the `handle_error(ctx, SodagunError { code, message }) -> !` pattern rather than `?` / `Result` propagation, so error codes and exit semantics stay explicit
- `sandbox attach` and `sandbox exec` exit with the inner process's exit code on success (via `std::process::exit()`), not a fixed code
- `build_guest_invocation(cmd, args, env, login)` centralizes the in-guest shell wrapper for both `attach` and `exec`: it returns a direct `(cmd, args)` invocation when `env` is empty and `login` is false, otherwise wraps in `sh [-l] -c 'export K=V; …; exec "$0" "$@"' cmd args`. Env values are POSIX single-quote-escaped by `shell_single_quote` before embedding; only `--env` keys are interpolated raw, so `validate_env_kv` rejects control chars in the key (and value) up front
- Async SDK calls are bridged to the synchronous handlers with the shared `util::get_runtime()` runtime; the `*_async` functions own all `.await`s and are private to their command module
- `util::map_sandbox_err()` maps `SandboxNotFound` → `SANDBOX_NOT_FOUND` (else `SANDBOX_ERROR`); `util::map_snapshot_err()` maps `SnapshotNotFound` → `SNAPSHOT_NOT_FOUND` (else `SNAPSHOT_ERROR`)
- Stop/wait is delegated to the SDK: `stop`/`remove` call `SandboxHandle::stop_with_timeout(timeout)` to send a graceful shutdown and wait for the sandbox to halt. The `--no-wait` path uses `tokio::spawn` to fire the stop off without awaiting it
- `snapshot create` builds the ephemeral sandbox with `.patch(...)`: the setup script is patched to `/setup-assets/_setup` (mode `0o755`; `_setup` = `config::SETUP_SCRIPT_NAME`, leading underscore avoids colliding with user `setup_files`), and each `setup_files` entry to `/setup-assets/<name>`. The script runs directly via `exec_stream("/setup-assets/_setup", …)`. Before snapshotting it runs `sync`, then `stop_and_wait()`. The resulting snapshot carries labels `created_by=sodagun`, `repo_path`, `setup_hash` (the 12-char base64url suffix of the name), and `source_image` — `repo_path` is what `snapshot clean` filters on

## Dependencies

Runtime: `clap` (derive), `git2` (`vendored-libgit2`), `microsandbox` (0.4), `serde` + `serde_json`, `toml`, `tokio` (`rt-multi-thread`, `time`), `uuid` (v4), `chrono` (`clock`), `colored`, `sha2` (0.11), `base64` (0.22), `sysinfo` (0.33, `system` feature only)
Dev: `assert_cmd`, `predicates`, `tempfile`, `automod`
0.x dependencies are pinned to a minor version (`sha2 = "0.11"`, `base64 = "0.22"`, `sysinfo = "0.33"`).
Supply chain: `cargo-deny` + `cargo-audit` wired into pre-commit and `make audit`; `Cargo.lock` is committed. `deny.toml` allows additional permissive licenses (ISC, BSD-3-Clause, 0BSD, CDLA-Permissive-2.0, etc.) and ignores specific advisories pulled in by `microsandbox` transitive deps; `make audit` mirrors those ignores with `--ignore` flags.
