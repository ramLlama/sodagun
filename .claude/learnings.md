# Learnings

Append-only log of non-obvious lessons learned while working on sodagun. Newest entries at the bottom; timestamp each batch.

## 2026-06-11 — sandbox git access + reliability fixes

- **macOS GUI/daemon-spawned processes inherit a 256 soft fd limit.** Sandbox VMs open an fd per virtiofs share/disk/socket; once `git_access` added ~9 mounts, VM boot SIGABRT'd — but only when sodagun was launched from Emacs. Fixed by raising soft `RLIMIT_NOFILE` to the hard limit at startup. Repro-harness trap: `ulimit -n` clamps the *hard* limit too (unraisable without root); use `ulimit -S -n 256` to mimic the GUI condition.
- **libgit2 writes the worktree `commondir` as an absolute host path** (git CLI writes `../..`), and some git codepaths read that file even when `GIT_COMMON_DIR` is set — a guest seeing only the mounts died with `fatal: Invalid path '/Users'` *after* a clean `GIT_TRACE_SETUP`. Normalize at creation.
- **`GIT_DIR`/`GIT_COMMON_DIR`/`GIT_WORK_TREE` env beats path mirroring** for guest git: the `.git` material can mount anywhere (`<working_dir>.git`), and guest git never consults the on-disk pointer files. But *host* git still reads them, so under the `data` policy the pointer files (`commondir`, `gitdir`, the worktree's `.git` file) must be pinned read-only — a guest rewriting any of them aims host git at attacker-controlled config/hooks (host code execution).
- **Nested/overlapping virtiofs bind mounts work, including read-only file pins inside rw mounts** (verified empirically; undocumented in microsandbox docs). Mount order must be parent before nested child.
- **`host.microsandbox.internal` reaches host `127.0.0.1`-bound services** (with a matching `allow@host` rule) — host servers don't need to bind `0.0.0.0`.
- **microsandbox sandboxes accept ONE concurrent connection** (`Sandbox::start` errors `already running` for the second client), and a killed client leaks the slot until remove/start. Drives the baton-side design (everything rides one attach connection).
- **Failed starts orphan SDK-side sandbox records**: `start` reserves the name, rolls back its own metadata on failure, but the SDK record survives — blocking retries with "already exists" and (before the fix) being unremovable since `remove` keyed off the rolled-back metadata.
- **JSON error output must carry the message** — emitting only the code left callers (baton) completely blind to causes; the fd-limit bug took an extra round-trip to diagnose purely because of this.
- **The derived snapshot name hashes the setup script AND `setup_files`** (`Cargo.toml`/`Cargo.lock`/`rust-toolchain.toml` here) — any dependency bump invalidates the snapshot; run `sodagun snapshot create` before the next sandbox start.
- **`cargo install --path .` ignores `Cargo.lock`** — a fresh resolve pulled a newer microsandbox with breaking API changes. Always `cargo install --path . --locked`.
- **Secrets-as-placeholders (`$MSB_*`) are not expanded by consumers like Claude** — credentials that a program reads from env must be injected as real env values (`[sandbox.env.*] value_from_cmd`) until custom placeholder expansion exists; note this trades away `allowed_hosts` gating.
