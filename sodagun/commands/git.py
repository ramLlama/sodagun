"""Git utility subcommands."""

from __future__ import annotations

import json
import tempfile
from pathlib import Path
from typing import Annotated, NoReturn
from uuid import uuid4

import pygit2
import typer

from sodagun.console import stderr as err_console
from sodagun.context import Context, OutputFormat

git_app = typer.Typer(name="git", help="Git utilities.", no_args_is_help=True)
_DEFAULT_DIR_PREFIX = Path(tempfile.gettempdir())


def _handle_error(ctx: Context, message: str, code: str) -> NoReturn:
    """Print error and exit with code 1. JSON mode outputs to stdout; text mode to stderr."""
    if ctx.output == OutputFormat.JSON:
        typer.echo(json.dumps({"status": "error", "code": code}))
    else:
        err_console.print(f"[bold red]Error[/bold red] [{code}]: {message}")
    raise typer.Exit(code=1)


@git_app.command("add-worktree")
def add_worktree(
    typer_ctx: typer.Context,
    repo_path: Annotated[Path, typer.Argument(help="Path to the git repository.")],
    branch_name: Annotated[str, typer.Argument(help="Name of the branch to create.")],
    dir_prefix: Annotated[
        Path,
        typer.Option("--dir-prefix", help="Parent directory for the worktree (default: /tmp)."),
    ] = _DEFAULT_DIR_PREFIX,
    base: Annotated[
        str,
        typer.Option("--base", help="Ref to base the new branch on."),
    ] = "origin/main",
) -> None:
    """Create a git worktree on a new branch, printing the resulting path."""
    assert isinstance(typer_ctx.obj, Context)
    ctx = typer_ctx.obj

    # pygit2.add_worktree requires the path to not exist yet
    worktree_path = dir_prefix / f"sodagun-wt-{repo_path.resolve().name}-{str(uuid4())[:8]}"

    # Open repo
    try:
        repo = pygit2.Repository(str(repo_path))
    except pygit2.GitError:
        _handle_error(ctx, f"Repository not found at {repo_path}", "REPO_NOT_FOUND")

    # Resolve base ref; revparse_single raises KeyError for unknown refs
    try:
        commit = repo.revparse_single(base).peel(pygit2.Commit)
    except KeyError:
        _handle_error(ctx, f"Base ref '{base}' not found", "BASE_NOT_FOUND")
    except pygit2.GitError:
        _handle_error(ctx, f"Could not resolve '{base}' to a commit", "BASE_INVALID")

    # Create branch
    try:
        branch = repo.branches.create(branch_name, commit)
    except pygit2.AlreadyExistsError:
        _handle_error(ctx, f"Branch '{branch_name}' already exists", "BRANCH_EXISTS")

    # Pre-check for worktree conflicts before calling add_worktree; avoids fragile string matching
    if worktree_path.exists() or branch_name in repo.list_worktrees():
        branch.delete()
        _handle_error(ctx, f"Worktree '{branch_name}' already exists", "WORKTREE_EXISTS")

    # Add worktree; roll back the branch on any failure to avoid leaving orphaned refs
    try:
        repo.add_worktree(branch_name, str(worktree_path), branch)
    except pygit2.GitError as exc:
        branch.delete()
        _handle_error(ctx, str(exc), "GIT_ERROR")

    if ctx.output == OutputFormat.JSON:
        typer.echo(json.dumps({"status": "ok", "worktree_path": str(worktree_path)}))
    else:
        typer.echo(str(worktree_path))
