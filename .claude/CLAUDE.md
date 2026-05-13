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

## Architecture

```
src/
  main.rs             # clap Cli struct, main(), dispatch
  context.rs          # OutputFormat (clap::ValueEnum, Default) + Context struct
  error.rs            # SodagunError, handle_error() -> !
  commands/
    mod.rs
    git.rs            # GitCommand sub-app; add_worktree logic
tests/
  integration/
    test_add_worktree.rs   # registered via [[test]] in Cargo.toml
Cargo.toml
deny.toml             # cargo-deny policy
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

## Dependencies

Runtime: `clap` (derive), `git2` (`vendored-libgit2`), `serde` + `serde_json`, `uuid` (v4), `colored`
Dev: `assert_cmd`, `predicates`, `tempfile`
Supply chain: `cargo-deny` + `cargo-audit` wired into pre-commit and `make audit`; `Cargo.lock` is committed

## Style

- Rust 2024 edition, no `unsafe`
- Error handling at the top level uses `handle_error(ctx, SodagunError { code, message }) -> !` (not `?` propagation) so every exit point carries an explicit error code
- `colored` crate for styled stderr: `"Error".red().bold()`
- Non-trivial functions get docstrings (`///`); comment blocks that do non-obvious work (e.g. why `file_name()` is used instead of `canonicalize()`)