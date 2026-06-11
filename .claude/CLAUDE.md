# sodagun

Rust CLI tool built with clap. Primary use case: utilities for AI agents working in git repos — creating worktrees and running them inside microsandbox VMs.

## Commands

```
sodagun [--output text|json] [--quiet] [--project-dir <path>] <subcommand>
```

Top-level flags (parsed by the `Cli` struct, must precede the subcommand — they are not true globals):
- `--output text|json` — output format. The selected `OutputFormat` is wrapped in a `Context` (with `quiet`) and passed by value into each handler.
- `--quiet` — suppress progress/log output (`ctx.log`/`ctx.warn` go to stderr and are silenced).
- `--project-dir <path>` — override the auto-detected project root. By default `find_project_dir()` walks up from CWD looking for `sodagun.toml` or `.git/`; it warns (unless `--quiet`) if `sodagun.toml` is not co-located with `.git`. The resolved `project_dir` is passed to the `git` and `snapshot` handlers.

### Workspace model

`git add-worktree` creates a **workspace rootdir** containing the git worktree plus a `sodagun.json` metadata file (`WorkspaceMetadata`, see `src/workspace.rs`). Downstream `sandbox` subcommands take a **workspace path** (the rootdir), read `sodagun.json` to find the bound sandbox, and enforce a strict 1:1 worktree↔sandbox mapping. The sandbox name equals the workspace rootdir's directory name.

`sodagun.json` schema (version 1): `{ version, repo_path, branch, created_at (ISO 8601 UTC), worktree_path, sandbox_name }`. `sandbox_name` is `null` until `sandbox start` reserves it, and cleared again by `sandbox remove`.

### Subcommands

- **`git add-worktree <branch-name> [repo-path]`** — creates a git worktree on a new branch inside a fresh workspace rootdir, writes `sodagun.json`, prints the rootdir path. Key opts: `--base <ref>` (default `origin/main`), `--dir-prefix <path>`. → Full reference: [commands-git-snapshot.md](commands-git-snapshot.md)
- **`sandbox start <workspace-path>`** — resolves + merges config, loads named network policies, creates a microsandbox via the SDK, persists the sandbox name, prints it. The worktree is bind-mounted at `working_dir`; boots from the derived snapshot when `[image]` declares a setup script. Key opts: `--config <path>`, `--net-rule <SPEC>`, `--net-default-egress/ingress`. → Full reference: [commands-sandbox.md](commands-sandbox.md)
- **`sandbox attach <workspace-path>`** — reconnects to the running sandbox and attaches an interactive TTY (login shell by default). Key opts: `--no-login`, `--env KEY=VALUE`, `-- CMD [ARGS...]`. → Full reference: [commands-sandbox.md](commands-sandbox.md)
- **`sandbox exec <workspace-path> <cmd> [args...]`** — runs `cmd` once in the sandbox (via a login shell by default) and returns its output; exits with the command's exit code. Key opts: `--no-login`, `--env KEY=VALUE`. → Full reference: [commands-sandbox.md](commands-sandbox.md)
- **`sandbox list`** — lists sodagun-managed sandboxes (names starting with `sodagun`) as a `NAME`/`STATUS` table or JSON. → Full reference: [commands-sandbox.md](commands-sandbox.md)
- **`sandbox stop <workspace-path>`** — sends a graceful shutdown signal and polls until halted. Key opts: `--stop-timeout-seconds <N>` (default 30), `--no-wait`. → Full reference: [commands-sandbox.md](commands-sandbox.md)
- **`sandbox remove <workspace-path>`** — stops the sandbox if running, then removes it and clears `sandbox_name`. Key opt: `--stop-timeout-seconds <N>` (default 30). → Full reference: [commands-sandbox.md](commands-sandbox.md)
- **`snapshot create`** — runs the `[image]` setup script in an ephemeral sandbox and snapshots it under a deterministic name `<sanitized-base>_<12-char-sha256>`. Key opts: `--config <path>`, `--force`. → Full reference: [commands-git-snapshot.md](commands-git-snapshot.md)
- **`snapshot remove`** — removes the derived snapshot for the `[image]` config. Key opts: `--config <path>`, `-f`/`--force`. → Full reference: [commands-git-snapshot.md](commands-git-snapshot.md)
- **`snapshot clean`** — removes all snapshots labeled `created_by=sodagun` AND `repo_path=<canonical project dir>`. Key opt: `--config <path>`. → Full reference: [commands-git-snapshot.md](commands-git-snapshot.md)

Full error codes and config invariants: [commands-sandbox.md](commands-sandbox.md) (sandbox/workspace), [commands-git-snapshot.md](commands-git-snapshot.md) (git/snapshot).

### `sodagun.toml` format

```toml
[image]
base_image = "debian"         # or base_snapshot = "name" (mutually exclusive; exactly one required)
setup_script = """
#!/usr/bin/env bash
set -e
apt-get install -y git
"""
# setup_script_path = "./setup.sh"  # alternative to inline setup_script (mutually exclusive)
setup_files = ["rust-toolchain.toml", "Cargo.toml"]  # paths relative to config file; injected into /setup-assets/ during snapshot create

[image.env]
HOME = "/root"                # env vars for the ephemeral build sandbox during snapshot create

[sandbox]
working_dir = "/workspace"  # default
memory_mb = 512             # default; type u32
cpus = 1                    # default; type u8 (serde rejects values > 255 at parse time)
git_access = "none"         # default; "none" | "data" | "full" — how much of the host .git the guest can touch (project > user > default)
volumes = ["~/.config/claude:/root/.config/claude:ro,noexec"]  # "host:guest" or "host:guest:OPTIONS" (comma-separated: ro, rw, noexec)

[sandbox.network]
policy = "none"   # built-in (none / allow-all / public-only) or a custom name from network-policy.d/. This repo's own sodagun.toml uses none (deps are pre-fetched into the snapshot via setup_files + cargo fetch)
# default_egress / default_ingress = "allow" | "deny"  # optional inline overrides
# [[sandbox.network.rules]]  # optional inline rules (same shape as named-policy rules; see below)

[sandbox.env]
TERM = "xterm-256color"          # plain string literal

[sandbox.env.MY_TOKEN]           # or a dynamic source (same shape as secrets)
value_from_cmd = "get-token.sh"  # or: value = "literal" / value_from_env = "HOST_VAR"

[sandbox.secrets.ANTHROPIC_API_KEY]
value_from_env = "ANTHROPIC_API_KEY"  # or: value = "literal" / value_from_cmd = "cmd"
allowed_hosts = ["api.anthropic.com"]
```

User-level config (`~/.config/sodagun/sodagun.toml`, `network-policy.d/`) and all `[image]`/`[sandbox]` key invariants: [commands-sandbox.md](commands-sandbox.md).

## Architecture

```
src/
  main.rs             # clap Cli struct (--output/--quiet/--project-dir), main(), raise_fd_limit(), find_project_dir(), dispatch
  context.rs          # OutputFormat (clap::ValueEnum, Default) + Context { output, quiet }; Context::log()/warn() (stderr, suppressed by --quiet)
  error.rs            # SodagunError { code, message } (#[derive(Debug)]), handle_error() -> ! (JSON envelope includes both code AND message)
  workspace.rs        # WorkspaceMetadata (sodagun.json: version, repo_path, branch, created_at, worktree_path, sandbox_name) + new()/read()/write()/set_sandbox_name()
  git_meta.rs         # linked-worktree git metadata resolvers: worktree_admin_dir() (from .git gitdir: pointer), common_git_dir() (from admin commondir), normalize_commondir() (rewrite admin commondir to relative ../..)
  config/
    mod.rs            # sodagun.toml parser; ImageConfig (incl. setup_files: Vec<SetupFile>, env), SetupFile { name, content }, GitAccess (None|Data|Full, lowercase serde), RawSandboxConfig (Option scalars, for parse+merge) / SandboxConfig (resolved), NetworkConfig (policy: Option<String>, default_egress/ingress, rules), NetworkRule, NamedPolicy, EnvValue (untagged Literal|Dynamic), ValueSource, SecretConfig, ConfigAction/Direction/Protocol, SETUP_SCRIPT_NAME, RESERVED_POLICY_NAMES, load_config(), load_image_config(), load_user_sandbox_config(), load_network_policies(), merge_sandbox_configs(), config_path(), snapshot_name(), parse_volume(), default_image_config()
    tests.rs          # #[cfg(test)] config unit tests
  util.rs             # dashify() name sanitizer + the microsandbox SDK↔sodagun layer: get_runtime() (OnceLock singleton), map_sandbox_err(), map_snapshot_err(), status_label()
  commands/
    mod.rs
    git.rs            # GitCommand sub-app; add_worktree logic (normalizes the new worktree's commondir via git_meta::normalize_commondir)
    sandbox/
      mod.rs          # SandboxCommand sub-app; start()/attach()/exec()/list()/stop()/remove() + private async impls, read_sandbox_name(); CliNetOptions (bundles the three --net-* overrides for start_async); build_guest_invocation() (shell-wrapper builder), shell_single_quote(), validate_env_kv()
      git_access.rs   # git_access_spec() → GitAccessSpec { mounts, env } implementing the GitAccess policy (mounts under <working_dir>.git + GIT_* env wiring)
      values.rs       # env/secret value resolution (run_value_cmd, validate_value_str, resolve_value_source, resolve_env_value, resolve_secret_value)
      network.rs      # network-policy building (apply_named_policy, apply_rule, commit_dest, to_sdk_action) + CLI net-rule SPEC parsing (parse_net_rule_value, parse_net_rule_spec)
      tests.rs        # #[cfg(test)] unit tests for the sandbox module
    snapshot.rs       # SnapshotCommand sub-app; create()/remove()/clean() + async impls, snapshot_build_resources(), SETUP_ASSETS_DIR
tests/
  integration.rs              # entry point: `mod integration { automod::dir!("tests/integration"); }` auto-discovers every file below as one shared test binary
  integration/
    test_add_worktree.rs
    test_sandbox_start.rs
    test_sandbox_lifecycle.rs
    test_snapshot.rs
    utils.rs            # sodagun()/sodagun_isolated() command builders, has_virtualization()/skip_without_virt() VM-test guards
scripts/
  require-virt.sh       # pre-push gate: blocks pushing from hosts that cannot boot VMs (where VM-boot tests self-skip)
Cargo.toml
deny.toml             # cargo-deny policy (permissive license allowances + microsandbox advisory ignores)
Makefile
.pre-commit-config.yaml
```

Key invariants (the most important few — the rest are in [architecture.md](architecture.md)):
- `handle_error()` returns `!` (Never type) — always calls `std::process::exit(1)` after printing; the Rust equivalent of Python's `NoReturn`. Top-level error handling uses the `handle_error(ctx, SodagunError { code, message }) -> !` pattern rather than `?` / `Result` propagation, so error codes and exit semantics stay explicit
- Text errors go to stderr (`eprintln!`); JSON errors go to stdout (`println!`) so `--output json` output is always parseable. The JSON error envelope carries both `code` and `message` (`{"status":"error","code":…,"message":…}`) so programmatic callers (e.g. baton) see the actual failure cause, not just the code. `get_runtime()` and `find_project_dir()` are the deliberate exceptions: their pre-command failures exit via plain stderr without a JSON envelope
- Async SDK calls are bridged to the synchronous handlers with the shared `util::get_runtime()` runtime; the `*_async` functions own all `.await`s and are private to their command module
- `build_guest_invocation(cmd, args, env, login)` centralizes the in-guest shell wrapper for both `attach` and `exec`: it returns a direct `(cmd, args)` invocation when `env` is empty and `login` is false, otherwise wraps in `sh [-l] -c 'export K=V; …; exec "$0" "$@"' cmd args` (env values POSIX single-quote-escaped)

Detailed invariants and dependency list: [architecture.md](architecture.md).

## Dev workflow

```bash
make all              # check-all + build-release-thin
make check-all        # fmt + lint + typecheck + test + audit
make test             # test-unit + test-integration
make test-unit        # cargo test --bin sodagun (in-source tests; no msb/git needed)
make test-integration # cargo test --test integration (spawns the binary; needs git; VM-boot tests skip without hardware virtualization)
make typecheck        # cargo check --all-targets
make lint             # cargo clippy --all-targets --all-features -- -D warnings
make fmt              # cargo fmt --all
make audit            # cargo deny check && cargo audit (with --ignore flags)
make build-release    # cargo build --release (fat LTO)
make build-release-thin  # cargo build --profile release-thin (thin LTO, faster link)
make build-debug      # cargo build
make install          # build-release, then cargo install --path . --profile release --locked
```

Two release profiles: `release` (fat LTO) and `release-thin` (`inherits = "release"`, `lto = "thin"`). `make all` builds the thin profile; `make install` uses the fat-LTO `release` profile.

Full testing conventions: [testing.md](testing.md).

## Style

- Rust 2024 edition, `unsafe` confined to two spots: `raise_fd_limit()` in `main.rs` (libc `getrlimit`/`setrlimit`) and the `#[cfg(test)]` env-var mutation in `config/tests.rs`
- Error handling at the top level uses `handle_error(ctx, SodagunError { code, message }) -> !` (not `?` propagation) so every exit point carries an explicit error code
- `colored` crate for styled stderr: `"Error".red().bold()`, `"warning:".yellow().bold()`
- Name sanitization goes through `util::dashify` rather than per-site `.replace(...)`
- Non-trivial functions get docstrings (`///`); comment blocks that do non-obvious work (e.g. why `canonicalize()` is used up front, or why the login-shell `exec "$0" "$@"` form is chosen)
