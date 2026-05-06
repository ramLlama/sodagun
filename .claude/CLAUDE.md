# sodagun

Python CLI tool built with Typer. Primary use case: utilities for AI agents working in git repos.

## Commands

```
sodagun [--output text|json] <subcommand>
```

`--output` is a global flag. All subcommands respect it.

### `git add-worktree <repo-path> <branch-name>`

Creates a git worktree on a new branch, prints the resulting path to stdout.

Options:
- `--base <ref>` — branch point (default: `origin/main`)
- `--dir-prefix <path>` — parent dir for the worktree (default: system temp dir)

Worktree path: `<dir-prefix>/sodagun-wt-<reponame>-<uuid8>`

JSON success: `{"status": "ok", "worktree_path": "..."}`
JSON error: `{"status": "error", "code": "<CODE>"}`

Error codes: `REPO_NOT_FOUND`, `BASE_NOT_FOUND`, `BASE_INVALID`, `BRANCH_EXISTS`, `WORKTREE_EXISTS`, `GIT_ERROR`

## Architecture

```
sodagun/
  cli.py              # app entry point; registers sub-apps; sets ctx.obj = Context(...)
  context.py          # OutputFormat (StrEnum) + Context dataclass
  console.py          # shared Rich Console instances: stdout, stderr
  commands/
    git.py            # git_app Typer sub-app; add-worktree command
```

Key invariants:
- `ctx.obj` is always a `Context` instance by the time any subcommand runs
- `_handle_error()` is `NoReturn` -- always raises `typer.Exit(code=1)` after printing
- Text errors go to stderr; JSON errors go to stdout (so `--output json` output is always parseable)
- `pygit2.add_worktree` requires the target path to not pre-exist
- Branch is rolled back (`branch.delete()`) if worktree creation fails
- `revparse_single` raises `KeyError` for unknown refs (not `GitError`) -- caught separately

## Dev workflow

```bash
make all        # format + lint + typecheck + test
make test       # pytest only
make typecheck  # mypy sodagun/
make lint       # ruff check
make format     # ruff format
```

All commands run via `uv run`. The project venv is `.venv/`.

## Testing conventions

- `tests/unit/` -- pure logic tests; pygit2 is mocked
- `tests/integration/` -- real pygit2 repo created in `tmp_path`
- Unit test classes use an `autouse=True` fixture that patches `pygit2.Repository` and yields the constructor mock
  - `mock_repo.return_value` is the repo instance
  - `mock_repo.side_effect = GitError(...)` simulates a failing constructor
  - `mock_repo.return_value.<method>.side_effect = ...` simulates instance-level failures
- Integration fixture creates a repo with one commit and an `origin/main` remote ref

## Dependencies

Runtime: `typer`, `rich`, `pygit2`
Dev: `mypy`, `ruff`, `pytest`, `pytest-cov`, `pre-commit`

## Style

- Python 3.14+, strict mypy
- `from __future__ import annotations` in all modules
- `X | None` over `Optional[X]`; `enum.StrEnum` over `str, enum.Enum`
- Explicit length checks (`if len(x) == 0`) over boolean coercion (`if not x`)
- Non-trivial functions get docstrings; comment blocks that do non-obvious work
