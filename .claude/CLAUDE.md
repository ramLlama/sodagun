# sodagun

Rust CLI tool built with clap. Primary use case: utilities for AI agents working in git repos — creating worktrees and running them inside microsandbox VMs.

## Commands

```
sodagun [--output text|json] [--quiet] [--project-dir <path>] <subcommand>
```

Top-level flags (parsed by the `Cli` struct, must precede the subcommand — they are not true globals):
- `--output text|json` — output format. The selected `OutputFormat` is wrapped in a `Context` (with `quiet`) and passed by value into each handler.
- `--quiet` — suppress progress/log output (`ctx.log` goes to stderr and is silenced).
- `--project-dir <path>` — override the auto-detected project root. By default `find_project_dir()` walks up from CWD looking for `sodagun.toml` or `.git/`; it warns (unless `--quiet`) if `sodagun.toml` is not co-located with `.git`. The resolved `project_dir` is passed to the `git` and `sandbox` handlers.

### Workspace model

`git add-worktree` creates a **workspace rootdir** containing the git worktree plus a `sodagun.json` metadata file (`WorkspaceMetadata`, see `src/workspace.rs`). Downstream `sandbox` subcommands take a **workspace path** (the rootdir), read `sodagun.json` to find the bound sandbox, and enforce a strict 1:1 worktree↔sandbox mapping. The sandbox name equals the workspace rootdir's directory name.

`sodagun.json` schema (version 1): `{ version, repo_path, branch, created_at (ISO 8601 UTC), worktree_path, sandbox_name }`. `sandbox_name` is `null` until `sandbox start` reserves it, and cleared again by `sandbox remove`.

### `git add-worktree <branch-name> [repo-path]`

Creates a git worktree on a new branch inside a fresh workspace rootdir, writes `sodagun.json`, and prints the rootdir path to stdout.

Options:
- `repo-path` — positional; path to the git repo (default: auto-detected `project_dir`)
- `--base <ref>` — branch point (default: `origin/main`)
- `--dir-prefix <path>` — parent dir for the rootdir (default: system temp dir)

Rootdir: `<dir-prefix>/sodagun_<repo>_<branch>_<uuid8>` (note: `_`-delimited; `<repo>` and `<branch>` components are run through `util::dashify`, which collapses `/ _ : @` and space to `-`, keeping `_` unambiguous as the separator). The worktree itself lives at `<rootdir>/<sanitized-branch>`.

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

### `sandbox start <workspace-path>`

Reads `sodagun.json` from the workspace, then resolves the project config (explicit `--config` > `<worktree>/sodagun.toml` > `<repo_path>/sodagun.toml` > built-in defaults), merges it with the user-level `~/.config/sodagun/sodagun.toml` via `merge_sandbox_configs()`, loads and merges `[registry]` config and user `[image]` overrides, loads any custom named network policies, creates a microsandbox via the SDK, persists the sandbox name into `sodagun.json`, and prints the sandbox name. The worktree is bind-mounted at the configured `working_dir`. When `[image]` declares a `dockerfile`, `start()` computes the OCI tag via `dockerfile_image_tag()` and `start_async()` boots from it via `builder.image(tag)` (with `builder.registry(|r| r.insecure())` when configured); a `MicrosandboxError::ImageNotFound` hints to run `sandbox create-image` first.

Options:
- `--config <path>` — config file path (overrides the resolution chain)

The sandbox name is reserved in `sodagun.json` *before* launch, then cleared on launch failure (rollback). Erroring if a sandbox is already recorded yields `SANDBOX_ALREADY_STARTED`.

JSON success: `{"status": "ok", "sandbox_name": "..."}`
JSON error: `{"status": "error", "code": "<CODE>"}`

### `sandbox attach <workspace-path>`

Reads the sandbox name from `sodagun.json`, reconnects to the running sandbox (`Sandbox::start`), and attaches an interactive TTY shell. By default attaches a login shell (`/bin/sh -l`) so profile files are sourced; `--no-login` uses `attach_shell()` instead. On a normal session end, exits with the shell's exit code via `std::process::exit()`. Only emits `SANDBOX_ERROR` on infrastructure failure.

Options:
- `--no-login` — skip the login shell

### `sandbox exec <workspace-path> <cmd> [args...]`

Reads the sandbox name from `sodagun.json`, connects (`Sandbox::start`), runs `cmd` once, and returns its output. Exits with the command's exit code.

Options:
- `--no-login` — run `cmd` directly instead of through a login shell

By default the command is run through a login shell so profiles/PATH (e.g. `/root/.cargo/bin`) are sourced: `sh -l -c 'exec "$0" "$@"' <cmd> <args>`. The `exec` *replaces* the shell in place with the real command (no nested shell), preserving argv exactly without re-quoting (`cmd` is `$0`, args are `$@`).

Text success: writes captured stdout/stderr to the corresponding streams.
JSON success: `{"status": "ok", "exit_code": N, "stdout": "...", "stderr": "..."}`

### `sandbox list`

Lists sodagun-managed sandboxes via `Sandbox::list()`, filtered to names starting with `sodagun` (the `sodagun_<...>` worktree sandboxes). When other microsandbox VMs are filtered out, logs `N non-sodagun sandbox(es) hidden; run \`msb list\`…` to stderr (so JSON on stdout stays clean).

Text success: aligned `NAME` / `STATUS` table (status lowercased).
JSON success: `{"status": "ok", "sandboxes": [{"name": "...", "status": "running"}, ...]}`

### `sandbox stop <workspace-path>`

Reads the sandbox name from `sodagun.json` and sends a graceful shutdown signal via `Sandbox::get(name)` → `handle.stop()`.

Options:
- `--stop-timeout-seconds <N>` — seconds to poll for the sandbox to reach `stopped`/`crashed` (default: 30)
- `--no-wait` — return immediately after sending the stop signal without polling

Text success: `"Stopped."` (or `"Stop signal sent."` with `--no-wait`)
JSON success: `{"status": "ok"}`

### `sandbox remove <workspace-path>`

Reads the sandbox name from `sodagun.json`. If the sandbox is still running, sends a stop signal and polls until it halts before `Sandbox::remove(name)`. Clears `sandbox_name` in `sodagun.json` on success.

Options:
- `--stop-timeout-seconds <N>` — seconds to wait for the implicit stop phase (default: 30)

Text success: `"Removed."`
JSON success: `{"status": "ok"}`

### `sandbox create-image`

Builds an OCI image from a `Dockerfile` declared in `[image].dockerfile`, then pushes it to the configured registry. Loads the project `[image]` and `[registry]` config, merges in the user-level `[registry]` config (`merge_registry_configs`) and user `[image]` overrides (`merge_user_image_config`: `namespace_repository`, `version`), and computes the OCI tag via `dockerfile_image_tag()`.

Options:
- `--config <path>` — config file path (default: `<project-dir>/sodagun.toml`)
- `--force` — rebuild/repush even if the image already exists in the registry

Flow: unless `--force`, checks existence with `podman manifest inspect <tag>` (skips the build if it succeeds); otherwise runs `podman build -f <dockerfile> -t <tag> <context_dir>` then `podman push <tag>`. The Dockerfile's parent directory is the build context.

JSON success: `{"status": "ok", "image_tag": "...", "already_existed": false}`
JSON error: `{"status": "error", "code": "<CODE>"}`

Error codes: `CONFIG_NOT_FOUND`, `CONFIG_INVALID`, `PODMAN_ERROR`

- `PODMAN_ERROR` — `podman build` or `podman push` failed (non-zero exit or spawn failure)

### Sandbox / workspace error codes

`WORKSPACE_NOT_FOUND`, `WORKSPACE_INVALID`, `WORKTREE_NOT_FOUND`, `CONFIG_NOT_FOUND`, `CONFIG_INVALID`, `SANDBOX_NOT_STARTED`, `SANDBOX_ALREADY_STARTED`, `SANDBOX_NOT_FOUND`, `SANDBOX_ERROR`, `PODMAN_ERROR`

- `WORKSPACE_NOT_FOUND` — no `sodagun.json` in the given rootdir (was it created by sodagun?)
- `WORKSPACE_INVALID` — `sodagun.json` is malformed, unreadable, or fails to serialize/write
- `WORKTREE_NOT_FOUND` — the worktree path recorded in `sodagun.json` does not exist or is not a directory
- `CONFIG_NOT_FOUND` — `sodagun.toml` missing from the config path
- `CONFIG_INVALID` — malformed TOML (incl. user `sodagun.toml` / `network-policies.toml`); in `[image]`: without `dockerfile`, missing `base_image`/`base_snapshot` (or both set together); `dockerfile` set together with `base_image` or `base_snapshot`; `dockerfile` set but `namespace_repository` absent; `dockerfile` path missing or not a file; `registry.host` absent when `dockerfile` is used (checked at `dockerfile_image_tag()` time); env/secret key conflict (after merge); `cpus` out of `u8` range; bad volume format; `$HOME` not set for `~` expansion; unresolvable `value_from_env`; `value_from_cmd` exits non-zero; a resolved env/secret value containing control characters; not exactly one of `value`/`value_from_env`/`value_from_cmd` set; unknown network policy name; a reserved policy name redefined in `network-policies.toml`; the old `[sandbox.network].mode` key (rejected via `deny_unknown_fields`); non-UTF-8 paths
- `SANDBOX_NOT_STARTED` — workspace has no sandbox recorded in `sodagun.json` (emitted by `attach`/`exec`/`stop`/`remove` when `sandbox_name` is null)
- `SANDBOX_ALREADY_STARTED` — `sandbox start` called on a workspace that already has a sandbox recorded
- `SANDBOX_NOT_FOUND` — named sandbox does not exist (maps `MicrosandboxError::SandboxNotFound`)
- `SANDBOX_ERROR` — microsandbox SDK failure (runtime creation, `create_detached`, `start`, `attach`/`exec`, stop/remove ops, stop timeout)
- `WORKSPACE_INVALID` is also emitted by `sandbox start` when the workspace path has no directory name (sandbox name can't be derived)

### `sodagun.toml` format

```toml
[image]
base_image = "debian"         # or base_snapshot = "name" (mutually exclusive; exactly one required when no dockerfile)
# dockerfile = "./Dockerfile"  # build an OCI image via `sandbox create-image` (mutually exclusive with base_image/base_snapshot); path relative to config file
# namespace_repository = "myorg/myrepo"  # required when dockerfile is set
# version = "1"                # optional; defaults to "1" at tag-compute time

[registry]                     # required when dockerfile is used
host = "registry.example.com"  # required when dockerfile is used (checked at tag-compute time)
# insecure = true              # optional; allow an insecure/HTTP registry

[sandbox]
working_dir = "/workspace"  # default
memory_mb = 512             # default; type u32
cpus = 1                    # default; type u8 (serde rejects values > 255 at parse time)
volumes = ["~/.config/claude:/root/.config/claude:ro,noexec"]  # "host:guest" or "host:guest:OPTIONS" (comma-separated: ro, rw, noexec)

[sandbox.network]
policy = "none"   # built-in (none / allow-all / public-only) or a custom name from network-policies.toml. This repo's own sodagun.toml uses none (deps are baked into the image via the Dockerfile)
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

Two optional **user-level** config files live under `$XDG_CONFIG_HOME/sodagun/` (falling back to `$HOME/.config/sodagun/`; resolved by `config_path(filename)`):

`~/.config/sodagun/sodagun.toml` — a user-level config supporting a `[sandbox]` section, a `[registry]` section, and an `[image]` section (the latter only honoring `namespace_repository` and `version` — other `[image]` fields are ignored at the user level). Loaded by `load_user_sandbox_config()` / `load_user_registry_config()` / `load_user_image_config()`. The `[sandbox]` section is merged with the project `[sandbox]` via `merge_sandbox_configs()`:
- `volumes`: user first, then project appended
- `env` / `secrets`: union; project wins on key conflict
- Scalars (`working_dir`, `memory_mb`, `cpus`): project > user > built-in default
- `network.policy` / `default_egress` / `default_ingress`: project > user; `network.rules`: user inline first, then project inline

The `[registry]` section is merged with the project `[registry]` via `merge_registry_configs()` (project wins per-field); the `[image]` overrides via `merge_user_image_config()` (project `namespace_repository`/`version` win when set, else user value).

`~/.config/sodagun/network-policies.toml` — custom named network policies (loaded by `load_network_policies()`). Each top-level table is a policy name:
```toml
[my-policy]
default_egress = "deny"    # or "allow"; optional
default_ingress = "allow"  # optional

[[my-policy.rules]]
direction = "egress"       # egress | ingress | any
action = "allow"           # allow | deny
destination = "api.example.com"   # domain, IP, CIDR, or one of: public/private/host/loopback/link_local/metadata/multicast/any
protocol = "tcp"           # tcp | udp; optional
ports = [443]              # optional
```
The built-in names in `RESERVED_POLICY_NAMES` (`none`, `allow-all`, `public-only`) are always available and **cannot** be redefined in this file (`CONFIG_INVALID` if attempted).

Note: the `[image]` table accepts only `base_image`, `base_snapshot`, `dockerfile`, `namespace_repository`, and `version`; the `[registry]` table accepts `host` and `insecure`.

`[image]` key invariants:
- `dockerfile` is mutually exclusive with both `base_image` and `base_snapshot` (`CONFIG_INVALID` if set together)
- When `dockerfile` is absent, exactly one of `base_image` / `base_snapshot` is required; they are mutually exclusive
- When `dockerfile` is set, `namespace_repository` is required (parse-time `CONFIG_INVALID` if absent) and the `dockerfile` path must exist and be a file (parse-time `CONFIG_INVALID` otherwise); it is resolved relative to the config file at load time
- `registry.host` is required whenever `dockerfile` is used — validated at `dockerfile_image_tag()` call time (not parse-time), so a `[registry]` with a missing `host` only fails once a dockerfile build/start is attempted
- The OCI image tag is computed by `dockerfile_image_tag(image_config, registry, dockerfile_bytes)`: deterministic given the same registry host, `namespace_repository`, `version` (defaulting to `"1"`), and Dockerfile contents. Changing the Dockerfile contents or the `version` changes the tag

Sandbox key invariants:
- `[image]` section is required when `sodagun.toml` exists; `[sandbox]` is optional (all fields have defaults). When no project config is found anywhere, `sandbox start` falls back to `default_image_config()` (alpine:latest, no setup) + `RawSandboxConfig::default()` for the project side — which is still merged with the user-level config, so user `[sandbox]` settings apply even without a project config
- `load_config()` returns a `RawSandboxConfig` (all scalars `Option`); the resolved `SandboxConfig` is produced only by `merge_sandbox_configs(user, project)`. The user config comes from `load_user_sandbox_config()`
- A key may not appear in both `[sandbox.env]` and `[sandbox.secrets]` — validated in `merge_sandbox_configs` (on the *merged* result), not `load_config`
- Network policy is named, not a mode: `[sandbox.network].policy` selects a built-in (`none` → `default_deny()`, `allow-all` → `default_allow()`, `public-only` → hand-built to mirror `NetworkPolicy::public_only()`) or a custom policy from `network-policies.toml`. Built-ins are resolved first and shadow any same-named custom policy. An unknown name is `CONFIG_INVALID`; the error shows the policies-file path if the file exists, else the built-in list. `default_egress`/`default_ingress`/`rules` (inline or from the named policy) layer on via `apply_named_policy()` + `apply_rule()`
- `cpus` is `u8` so serde rejects out-of-range values at parse time with `CONFIG_INVALID`
- Volume strings are Docker-style `"host:guest"` or `"host:guest:OPTIONS"`, where `OPTIONS` is a comma-separated list of `ro` (read-only), `rw` (explicit read-write; no-op), and `noexec` (disable direct execution from the mount). An unknown option is `CONFIG_INVALID`. `config::parse_volume` returns `(PathBuf, String, MountFlags)` where `MountFlags { readonly: bool, noexec: bool }`. Tilde (`~`) expansion to `$HOME` happens at launch time (`config::parse_volume`), not config-parse time
- `[sandbox.env]` values are either a plain string (`EnvValue::Literal`) or a dynamic `ValueSource` (`value` / `value_from_env` / `value_from_cmd`) — the same three sources as secrets. Exactly one source must be set (enforced at launch, not parse). `value_from_env` / `value_from_cmd` are resolved at launch time, not config-parse time, so values stay out of the parsed struct
- `value_from_cmd` runs via `sh -c <cmd>` on the host; non-zero exit is `CONFIG_INVALID`; stdout is trimmed before use
- All resolved env/secret values are checked by `validate_value_str()`: a value containing any control character (newline, CR, NUL, …) is `CONFIG_INVALID` (prevents a SIGABRT in the microsandbox VM)
- Secret `allowed_hosts` entries containing `*` use `allow_host_pattern`; others use `allow_host`
- Async work runs on a process-wide lazy `tokio` multi-thread runtime (`util::get_runtime`, a `OnceLock` singleton); failing to build it exits directly via stderr (no JSON envelope)

## Architecture

```
src/
  main.rs             # clap Cli struct (--output/--quiet/--project-dir), main(), find_project_dir(), dispatch
  context.rs          # OutputFormat (clap::ValueEnum, Default) + Context { output, quiet }; Context::log() (stderr, suppressed by --quiet)
  error.rs            # SodagunError (#[derive(Debug)]), handle_error() -> !
  workspace.rs        # WorkspaceMetadata (sodagun.json: version, repo_path, branch, created_at, worktree_path, sandbox_name) + new()/read()/write()/set_sandbox_name()
  config/             # sodagun.toml parser (submodule). ImageConfig { base_image, base_snapshot, dockerfile: Option<PathBuf>, namespace_repository: Option<String>, version: Option<String> }, RegistryConfig { host: Option<String>, insecure: Option<bool> }, UserImageConfig { namespace_repository, version }, RawSandboxConfig (Option scalars, for parse+merge) / SandboxConfig (resolved), NetworkConfig (policy: Option<String>, default_egress/ingress, rules), NetworkRule, NamedPolicy, EnvValue (untagged Literal|Dynamic), ValueSource, SecretConfig, ConfigAction/Direction/Protocol, RESERVED_POLICY_NAMES; load_config() (→ ImageConfig + RawSandboxConfig), load_registry_config(), load_user_sandbox_config(), load_user_registry_config(), load_user_image_config()/load_user_image_config_from_path(), load_network_policies(), merge_sandbox_configs(), merge_registry_configs(), merge_user_image_config(), dockerfile_image_tag(), config_path(), parse_volume(), default_image_config()
  util.rs             # dashify() name sanitizer + the microsandbox SDK↔sodagun layer: get_runtime() (OnceLock singleton), map_sandbox_err(), status_label()
  commands/
    mod.rs            # pub mod git; pub mod sandbox;
    git.rs            # GitCommand sub-app; add_worktree logic
    sandbox.rs        # SandboxCommand sub-app; run(project_dir); start()/attach()/exec()/list()/stop()/remove()/create-image() + private async impls, read_sandbox_name(); value resolution (run_value_cmd, validate_value_str, resolve_value_source, resolve_env_value, resolve_secret_value) + network-policy building (apply_named_policy, apply_rule, commit_dest, to_sdk_action)
tests/
  integration.rs              # entry point: `mod integration { automod::dir!("tests/integration"); }` auto-discovers every file below as one shared test binary
  integration/
    test_add_worktree.rs
    test_sandbox_start.rs
    test_sandbox_lifecycle.rs
    test_sandbox_create_image.rs
Cargo.toml
deny.toml             # cargo-deny policy (permissive license allowances + microsandbox advisory ignores)
Makefile
.pre-commit-config.yaml
```

Key invariants:
- Top-level flags (`--output`/`--quiet`/`--project-dir`) must precede the subcommand (not true globals); `Context { output, quiet }` is constructed in `main()` and passed by value into each handler. `project_dir` is resolved by `find_project_dir()` and passed to the `git`/`sandbox` handlers (`sandbox::run(project_dir)`, mirroring `git::run`)
- `handle_error()` returns `!` (Never type) — always calls `std::process::exit(1)` after printing; the Rust equivalent of Python's `NoReturn`
- Text errors go to stderr (`eprintln!`); JSON errors go to stdout (`println!`) so `--output json` output is always parseable. `get_runtime()` and `find_project_dir()` are the deliberate exceptions: their pre-command failures exit via plain stderr without a JSON envelope
- `git2::Repository::worktree()` requires the target path to not pre-exist; the worktree name stored under `.git/worktrees/` is the dashified branch (git can't nest dirs there)
- Branch / rootdir / metadata are rolled back manually on any post-creation failure (no RAII guard yet)
- `repo.revparse_single()` returns `ErrorCode::NotFound` for unknown refs (equivalent to Python's `KeyError`) — caught separately from other git errors
- Top-level error handling uses the `handle_error(ctx, SodagunError { code, message }) -> !` pattern rather than `?` / `Result` propagation, so error codes and exit semantics stay explicit
- `sandbox attach` and `sandbox exec` exit with the inner process's exit code on success (via `std::process::exit()`), not a fixed code
- Async SDK calls are bridged to the synchronous handlers with the shared `util::get_runtime()` runtime; the `*_async` functions own all `.await`s and are private to their command module
- `util::map_sandbox_err()` maps `SandboxNotFound` → `SANDBOX_NOT_FOUND` (else `SANDBOX_ERROR`)
- Stop/wait is delegated to the SDK: `stop`/`remove` call `SandboxHandle::stop_with_timeout(timeout)` to send a graceful shutdown and wait for the sandbox to halt. The `--no-wait` path uses `tokio::spawn` to fire the stop off without awaiting it
- `sandbox create-image` shells out to `podman` (not the microsandbox SDK): `podman manifest inspect <tag>` for the existence check, then `podman build -f <dockerfile> -t <tag> <context_dir>` and `podman push <tag>`. A non-zero exit or spawn failure on any of these is `PODMAN_ERROR`. `sandbox start` boots a dockerfile image with `builder.image(tag)` (+ `builder.registry(|r| r.insecure())` when `registry.insecure` is set); `MicrosandboxError::ImageNotFound` from the SDK is mapped to a `SANDBOX_ERROR` whose message hints to run `sandbox create-image` first

## Dev workflow

```bash
make all              # check-all + build-release-thin
make check-all        # fmt + lint + typecheck + test + audit
make test             # cargo test
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

## Testing conventions

- Integration tests live under `tests/integration/` and are pulled in by `tests/integration.rs` via `automod::dir!` (wrapped in `mod integration { … }` so submodule paths resolve correctly). New files are auto-discovered — no `[[test]]` registration. They invoke the compiled binary through `assert_cmd::Command::cargo_bin("sodagun")`
- `test_add_worktree.rs` — end-to-end worktree tests. Helper `make_git_repo()` does `Repository::init`, one commit, and a `refs/remotes/origin/main` ref so `--base origin/main` resolves out of the box
- All integration test files use the `sodagun_isolated(xdg_tmp)` helper, which sets `XDG_CONFIG_HOME` to an empty tempdir so tests don't pick up the real user-level config files
- `test_sandbox_start.rs` — error-path tests (CONFIG_NOT_FOUND, malformed TOML, `[image]` validation errors, text + json) plus `start_creates_sandbox`, a happy-path test that boots a real sandbox (needs hardware virtualization — KVM or Apple Silicon hvf — to pass). Also covers the new `CONFIG_INVALID` paths: `value_from_cmd` non-zero exit, `value_from_cmd` multiline output (rejected), a trailing newline accepted (trimmed), unknown policy name with no policies file, and a reserved policy name redefined in `network-policies.toml`
- `test_sandbox_lifecycle.rs` — list (JSON shape + text header), and workspace-based stop/remove error paths. Helpers `make_workspace()` / `make_workspace_with_sandbox()` write a `sodagun.json` fixture directly. Includes happy-path tests (`stop_running_sandbox`, `remove_running_sandbox_implicit_stop`) that boot a real sandbox
- `test_sandbox_create_image.rs` — error-path tests for `sandbox create-image` (no hardware/podman needed): `CONFIG_NOT_FOUND`, no `dockerfile` in config, dockerfile path not found, dockerfile+`base_image` conflict, dockerfile+`base_snapshot` conflict, missing `namespace_repository`, missing `registry.host` (text and JSON)
- The happy-path tests above are **not** `#[ignore]`d — they run as part of `cargo test` and pass on a host with hardware virtualization (they will fail without it)
- `src/config/tests.rs` has `#[cfg(test)]` unit tests covering valid `[image]` configs, defaults, every `CONFIG_INVALID` / `CONFIG_NOT_FOUND` path, dockerfile config validation, `parse_volume` (including tilde expansion + the `$HOME`-unset path, which mutates the env var under a mutex), the rejection of the old `mode` field, `EnvValue`/`value_from_cmd` parsing, `merge_sandbox_configs` semantics, `merge_registry_configs`, `load_user_image_config` / `merge_user_image_config`, `dockerfile_image_tag` (determinism, content change, version change, format, missing-host error, missing-`namespace_repository` error), and `load_network_policies` (valid/malformed/reserved-name)
- `src/commands/sandbox.rs` has `#[cfg(test)]` unit tests for `apply_rule` (domain/CIDR) and `apply_named_policy` (from file, unknown built-in, unknown-in-file, built-in works with file present, and `public-only` matching the SDK preset)
- `src/util.rs` unit-tests `dashify`; `src/commands/git.rs` unit-tests the rootdir naming contract; `src/workspace.rs` unit-tests the `sodagun.json` roundtrip

## Dependencies

Runtime: `clap` (derive), `git2` (`vendored-libgit2`), `microsandbox` (0.4), `serde` + `serde_json`, `toml`, `tokio` (`rt-multi-thread`, `time`), `uuid` (v4), `chrono` (`clock`), `colored`, `sha2` (0.11), `base64` (0.22)
Dev: `assert_cmd`, `predicates`, `tempfile`, `automod`
0.x dependencies are pinned to a minor version (`sha2 = "0.11"`, `base64 = "0.22"`).
Supply chain: `cargo-deny` + `cargo-audit` wired into pre-commit and `make audit`; `Cargo.lock` is committed. `deny.toml` allows additional permissive licenses (ISC, BSD-3-Clause, 0BSD, CDLA-Permissive-2.0, etc.) and ignores specific advisories pulled in by `microsandbox` transitive deps; `make audit` mirrors those ignores with `--ignore` flags.

## Style

- Rust 2024 edition, no `unsafe` (the one exception: `#[cfg(test)]` env-var mutation in `config.rs` tests)
- Error handling at the top level uses `handle_error(ctx, SodagunError { code, message }) -> !` (not `?` propagation) so every exit point carries an explicit error code
- `colored` crate for styled stderr: `"Error".red().bold()`, `"warning:".yellow().bold()`
- Name sanitization goes through `util::dashify` rather than per-site `.replace(...)`
- Non-trivial functions get docstrings (`///`); comment blocks that do non-obvious work (e.g. why `canonicalize()` is used up front, or why the login-shell `exec "$0" "$@"` form is chosen)
