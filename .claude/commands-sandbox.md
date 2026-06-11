# sandbox command reference

## `sandbox start <workspace-path>`

Reads `sodagun.json` from the workspace, then resolves the project config (explicit `--config` > `<worktree>/sodagun.toml` > `<repo_path>/sodagun.toml` > built-in defaults), merges it with the user-level `~/.config/sodagun/sodagun.toml` via `merge_sandbox_configs()`, loads any custom named network policies, creates a microsandbox via the SDK, persists the sandbox name into `sodagun.json`, and prints the sandbox name. The worktree is bind-mounted at the configured `working_dir`. When `[image]` declares a setup script, it boots from the derived snapshot (which must already exist — see `snapshot create`).

Options:
- `--config <path>` — config file path (overrides the resolution chain)
- `--net-rule <SPEC>` — repeatable (and comma-separated per value); in-situ egress network rules appended *after* the config/named-policy rules. Format: `action@destination[:proto[:port]]` (e.g. `allow@host:tcp:9999`). `action` is `allow`/`deny`; `proto` is `tcp`/`udp`; direction is always egress. IPv6 literals are unsupported in the CLI (the `:` separator collides with them) — use `sodagun.toml` `[[sandbox.network.rules]]` for those. Since the policy is first-match-wins, appending keeps CLI rules effective.
- `--net-default-egress allow|deny` — override the default egress action from config (last-write-wins in the builder, so CLI beats config).
- `--net-default-ingress allow|deny` — override the default ingress action from config.

CLI net rules/defaults also **activate** the network policy even when the config selects no policy (i.e. they bootstrap a policy from nothing).

The sandbox name is reserved in `sodagun.json` *before* launch, then cleared on launch failure (rollback). Erroring if a sandbox is already recorded yields `SANDBOX_ALREADY_STARTED`.

JSON success: `{"status": "ok", "sandbox_name": "..."}`
JSON error: `{"status": "error", "code": "<CODE>"}`

## `sandbox attach <workspace-path>`

Reads the sandbox name from `sodagun.json`, reconnects to the running sandbox (`Sandbox::start`), and attaches an interactive TTY shell. By default attaches a login shell (`/bin/sh -l`) so profile files are sourced; `--no-login` uses `attach_shell()` instead. On a normal session end, exits with the shell's exit code via `std::process::exit()`. Only emits `SANDBOX_ERROR` on infrastructure failure.

Options:
- `--no-login` — skip the login shell
- `--env KEY=VALUE` — repeatable; inject extra environment variables into the in-guest command.
- `-- CMD [ARGS...]` — anything after `--` runs `CMD` via a PTY instead of the default login shell. Without `--`, behavior is unchanged (interactive shell).

When `--env` is given or an explicit command is specified, the invocation is wrapped via `build_guest_invocation` in `sh [-l] -c 'export K=V; …; exec "$0" "$@"'`; otherwise a plain login/non-login shell is attached.

## `sandbox exec <workspace-path> <cmd> [args...]`

Reads the sandbox name from `sodagun.json`, connects (`Sandbox::start`), runs `cmd` once, and returns its output. Exits with the command's exit code.

Options:
- `--no-login` — run `cmd` directly instead of through a login shell
- `--env KEY=VALUE` — repeatable; inject extra environment variables into the in-guest command.

By default the command is run through a login shell so profiles/PATH (e.g. `/root/.cargo/bin`) are sourced: `sh -l -c 'exec "$0" "$@"' <cmd> <args>`. The `exec` *replaces* the shell in place with the real command (no nested shell), preserving argv exactly without re-quoting (`cmd` is `$0`, args are `$@`). With `--env`, `build_guest_invocation` prepends `export K=V; …` (values POSIX single-quote-escaped) before the `exec`.

Text success: writes captured stdout/stderr to the corresponding streams.
JSON success: `{"status": "ok", "exit_code": N, "stdout": "...", "stderr": "..."}`

## `sandbox list`

Lists sodagun-managed sandboxes via `Sandbox::list()`, filtered to names starting with `sodagun` (covers `sodagun_<...>` worktree sandboxes and `sodagun-snap-<...>` ephemeral snapshot builders). When other microsandbox VMs are filtered out, logs `N non-sodagun sandbox(es) hidden; run \`msb list\`…` to stderr (so JSON on stdout stays clean).

Text success: aligned `NAME` / `STATUS` table (status lowercased).
JSON success: `{"status": "ok", "sandboxes": [{"name": "...", "status": "running"}, ...]}`

## `sandbox stop <workspace-path>`

Reads the sandbox name from `sodagun.json` and sends a graceful shutdown signal via `Sandbox::get(name)` → `handle.stop()`.

Options:
- `--stop-timeout-seconds <N>` — seconds to poll for the sandbox to reach `stopped`/`crashed` (default: 30)
- `--no-wait` — return immediately after sending the stop signal without polling

Text success: `"Stopped."` (or `"Stop signal sent."` with `--no-wait`)
JSON success: `{"status": "ok"}`

## `sandbox remove <workspace-path>`

Reads the sandbox name from `sodagun.json`. If the sandbox is still running, sends a stop signal and polls until it halts before `Sandbox::remove(name)`. Clears `sandbox_name` in `sodagun.json` on success.

When `sodagun.json` has no `sandbox_name`, `remove` falls back to the **derived** name (the workspace dir name): a failed `start` can orphan an SDK-side sandbox record after its metadata rollback, and `remove` is the only way to clear it. In that fallback path, a `SANDBOX_NOT_FOUND` from the SDK (i.e. nothing orphaned either — a never-started workspace) is remapped to the friendlier `SANDBOX_NOT_STARTED`.

Options:
- `--stop-timeout-seconds <N>` — seconds to wait for the implicit stop phase (default: 30)

Text success: `"Removed."`
JSON success: `{"status": "ok"}`

## Sandbox / workspace error codes

`WORKSPACE_NOT_FOUND`, `WORKSPACE_INVALID`, `WORKTREE_NOT_FOUND`, `CONFIG_NOT_FOUND`, `CONFIG_INVALID`, `GIT_ERROR`, `GIT_ACCESS_INVALID`, `SANDBOX_NOT_STARTED`, `SANDBOX_ALREADY_STARTED`, `SANDBOX_NOT_FOUND`, `SANDBOX_ERROR`

- `WORKSPACE_NOT_FOUND` — no `sodagun.json` in the given rootdir (was it created by sodagun?)
- `WORKSPACE_INVALID` — `sodagun.json` is malformed, unreadable, or fails to serialize/write
- `WORKTREE_NOT_FOUND` — the worktree path recorded in `sodagun.json` does not exist or is not a directory
- `CONFIG_NOT_FOUND` — `sodagun.toml` missing from the config path
- `CONFIG_INVALID` — malformed TOML (incl. user `sodagun.toml` / files in `network-policy.d/`); missing `base_image`/`base_snapshot` in `[image]`; both set together; `setup_script`+`setup_script_path` conflict; a missing/unreadable `setup_files` entry; a `setup_files` entry with a non-UTF-8 basename or the reserved name `_setup`; env/secret key conflict (after merge); `cpus` out of `u8` range; bad volume format; `$HOME` not set for `~` expansion; unresolvable `value_from_env`; `value_from_cmd` exits non-zero; a resolved env/secret value containing control characters; a `--env KEY=VALUE` whose key or value contains control characters (validated by `validate_env_kv`); a malformed `--net-rule` SPEC (missing `@`, bad action/protocol/port, empty destination); not exactly one of `value`/`value_from_env`/`value_from_cmd` set; unknown network policy name; a file in `network-policy.d/` whose stem is a reserved built-in name; the old `[sandbox.network].mode` key (rejected via `deny_unknown_fields`); non-UTF-8 paths
- `SANDBOX_NOT_STARTED` — workspace has no sandbox recorded in `sodagun.json` (emitted by `attach`/`exec`/`stop`/`remove` when `sandbox_name` is null)
- `SANDBOX_ALREADY_STARTED` — `sandbox start` called on a workspace that already has a sandbox recorded
- `SANDBOX_NOT_FOUND` — named sandbox does not exist (maps `MicrosandboxError::SandboxNotFound`)
- `SANDBOX_ERROR` — microsandbox SDK failure (runtime creation, `create_detached`, `start`, `attach`/`exec`, stop/remove ops, stop timeout)
- `WORKSPACE_INVALID` is also emitted by `sandbox start` when the workspace path has no directory name (sandbox name can't be derived)
- `GIT_ERROR` — (from `git_meta` resolvers, surfaced during `git_access` mount synthesis) the worktree's `.git` file is unreadable / has no `gitdir:` pointer, or a `commondir`/admin path fails to canonicalize
- `GIT_ACCESS_INVALID` — `git_access` mount synthesis failed: the admin dir has no name, a required `.git` subdir (e.g. `logs/`) can't be created, or a git mount host path is non-UTF-8

## User-level config files

Two optional **user-level** config files live under `$XDG_CONFIG_HOME/sodagun/` (falling back to `$HOME/.config/sodagun/`; resolved by `config_path(filename)`):

`~/.config/sodagun/sodagun.toml` — a user-level `[sandbox]` config (no `[image]` section; silently ignored if present). Loaded by `load_user_sandbox_config()` and merged with the project `[sandbox]` via `merge_sandbox_configs()`:
- `volumes`: user first, then project appended
- `env` / `secrets`: union; project wins on key conflict
- Scalars (`working_dir`, `memory_mb`, `cpus`, `git_access`): project > user > built-in default (`git_access` defaults to `none`)
- `network.policy` / `default_egress` / `default_ingress`: project > user; `network.rules`: user inline first, then project inline

`~/.config/sodagun/network-policy.d/<name>.toml` — custom named network policies (loaded by `load_network_policies()`). Each `.toml` file defines one policy; the policy name is the file stem. Files are loaded in alphabetical order:
```toml
# ~/.config/sodagun/network-policy.d/my-policy.toml
default_egress = "deny"    # or "allow"; optional
default_ingress = "allow"  # optional

[[rules]]
direction = "egress"       # egress | ingress | any
action = "allow"           # allow | deny
destination = "api.example.com"   # domain, IP, CIDR, or one of: public/private/host/loopback/link_local/metadata/multicast/any
protocol = "tcp"           # tcp | udp; optional
ports = [443]              # optional
```
The built-in names in `RESERVED_POLICY_NAMES` (`none`, `allow-all`, `public-only`) are always available and **cannot** be used as file stems (`CONFIG_INVALID` if attempted). Non-`.toml` files in the directory are ignored.

Note: snapshot-build sizing (memory/cpus for the ephemeral builder) is derived from the host, not from `[image]` — there are no `memory_mb`/`cpus` keys under `[image]`. The `[image]` table accepts only `base_image`, `base_snapshot`, `setup_script`, `setup_script_path`, `setup_files`, and `env`.

## `[image]` key invariants

- Exactly one of `base_image` / `base_snapshot` is required; they are mutually exclusive
- At most one of `setup_script` / `setup_script_path`; they are mutually exclusive
- `setup_script_path` is resolved relative to the config file at load time
- `setup_files` is a list of paths relative to the config file; each is resolved at load time into a `SetupFile { name, content }` (basename + raw bytes) and injected into `/setup-assets/<name>` via a patch during snapshot creation. A missing/unreadable entry, a non-UTF-8 basename, or the reserved basename `_setup` (= `config::SETUP_SCRIPT_NAME`, the slot the setup script itself occupies) is `CONFIG_INVALID`
- Snapshot name is `<sanitized-base>_<first-12-base64url-chars-of-sha256(script + setup_files)>`; setup file contents are hashed sorted by name, so the name is deterministic given the same base + script + setup_files. `dashify` produces the sanitized base. The hash covers the script bytes and setup_files (name + content), not the guest paths

## Sandbox key invariants

- `[image]` section is required when `sodagun.toml` exists; `[sandbox]` is optional (all fields have defaults). When no project config is found anywhere, `sandbox start` falls back to `default_image_config()` (alpine:latest, no setup) + `RawSandboxConfig::default()` for the project side — which is still merged with the user-level config, so user `[sandbox]` settings apply even without a project config
- `load_config()` returns a `RawSandboxConfig` (all scalars `Option`); the resolved `SandboxConfig` is produced only by `merge_sandbox_configs(user, project)`. The user config comes from `load_user_sandbox_config()`
- A key may not appear in both `[sandbox.env]` and `[sandbox.secrets]` — validated in `merge_sandbox_configs` (on the *merged* result), not `load_config`
- Network policy is named, not a mode: `[sandbox.network].policy` selects a built-in (`none` → `default_deny()`, `allow-all` → `default_allow()`, `public-only` → hand-built to mirror `NetworkPolicy::public_only()`) or a custom policy from `network-policy.d/`. Built-ins are resolved first and shadow any same-named custom policy. An unknown name is `CONFIG_INVALID`; the error shows the directory path if it exists, else the built-in list. `default_egress`/`default_ingress`/`rules` (inline or from the named policy) layer on via `apply_named_policy()` + `apply_rule()`
- `cpus` is `u8` so serde rejects out-of-range values at parse time with `CONFIG_INVALID`
- `git_access` (`"none"` | `"data"` | `"full"`, default `none`; `GitAccess` enum, lowercase serde) controls how much of the host repo's `.git` the guest can touch. A linked worktree's `.git` file points into the host repo, so guest git needs the host `.git` material. Rather than mirroring host paths, the shared `.git` is mounted at `<working_dir>.git` (e.g. `/workspace.git`) and git is wired via env injected at `sandbox start`: `GIT_DIR=<working_dir>.git/worktrees/<name>`, `GIT_COMMON_DIR=<working_dir>.git`, `GIT_WORK_TREE=<working_dir>`. Two git-config pairs are also injected via `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n`: `core.hooksPath=/dev/null` (host-installed hooks lack their tooling in-guest) and `gc.auto=0` (gc wants `packed-refs.lock` at the read-only `.git` top level). All synthesized `GIT_*` vars are an escape hatch: any matching `[sandbox.env]`/`[sandbox.secrets]` key **wins** over the synthesized value.
  - `none` (default): no mounts, no env; git commands fail inside the guest.
  - `data`: `.git` mounted **read-only**, with nested **read-write** mounts for `objects/`, `refs/`, `logs/` (created on demand so the mount source exists) and the worktree's admin dir. The admin dir's `commondir`/`gitdir` pointer files AND the worktree's own `.git` file are pinned **read-only** — host-side git reads those, and a guest rewriting them could aim host git at attacker-controlled config/hooks (host code execution). Threat model: the read-write surfaces are data-only.
  - `full`: the whole `.git` mounted **read-write** — the guest can edit `config`/`hooks/`, which execute on the **host** the next time git runs there. Only for trusted agents.
  - Host `~/.gitconfig` (if it exists) is mounted at `<working_dir>.gitconfig` (read-only under `data`, read-write under `full`) and wired via `GIT_CONFIG_GLOBAL` — guest homes differ from the host's, so this carries the user's commit identity into the guest.
  - Implemented in `commands/sandbox/git_access.rs` (`git_access_spec` → `GitAccessSpec { mounts, env }`); wired into the builder in `start_async` **after** the worktree mount (the `data` policy's `.git`-file pin layers over the worktree mount, and parents must precede nested children). Errors: `GIT_ERROR` / `GIT_ACCESS_INVALID`.
- Volume strings are Docker-style `"host:guest"` or `"host:guest:OPTIONS"`, where `OPTIONS` is a comma-separated list of `ro` (read-only), `rw` (explicit read-write; no-op), and `noexec` (disable direct execution from the mount). An unknown option is `CONFIG_INVALID`. `config::parse_volume` returns `(PathBuf, String, MountFlags)` where `MountFlags { readonly: bool, noexec: bool }`. Tilde (`~`) expansion to `$HOME` happens at launch time (`config::parse_volume`), not config-parse time
- `[sandbox.env]` values are either a plain string (`EnvValue::Literal`) or a dynamic `ValueSource` (`value` / `value_from_env` / `value_from_cmd`) — the same three sources as secrets. Exactly one source must be set (enforced at launch, not parse). `value_from_env` / `value_from_cmd` are resolved at launch time, not config-parse time, so values stay out of the parsed struct
- `value_from_cmd` runs via `sh -c <cmd>` on the host; non-zero exit is `CONFIG_INVALID`; stdout is trimmed before use
- All resolved env/secret values are checked by `validate_value_str()`: a value containing any control character (newline, CR, NUL, …) is `CONFIG_INVALID` (prevents a SIGABRT in the microsandbox VM)
- Secret `allowed_hosts` entries containing `*` use `allow_host_pattern`; others use `allow_host`
- Async work runs on a process-wide lazy `tokio` multi-thread runtime (`util::get_runtime`, a `OnceLock` singleton); failing to build it exits directly via stderr (no JSON envelope)
