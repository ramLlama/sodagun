"""Integration tests for git add-worktree using a real pygit2 repo."""

from __future__ import annotations

import json
from pathlib import Path

import pygit2
import pytest
from typer.testing import CliRunner

from sodagun.cli import app

runner = CliRunner()


@pytest.fixture()
def git_repo(tmp_path: Path) -> pygit2.Repository:
    """Create a bare-minimum git repo with one commit so worktrees can be added."""
    repo_path = tmp_path / "repo"
    repo_path.mkdir()
    repo = pygit2.init_repository(str(repo_path))

    # Commit a file so HEAD exists and branches can be created
    (repo_path / "README").write_text("hello")
    index = repo.index
    index.add("README")
    index.write()
    tree = index.write_tree()
    sig = pygit2.Signature("Test", "test@example.com")
    oid = repo.create_commit("refs/heads/main", sig, sig, "initial commit", tree, [])

    # Make origin/main resolve (needed for the default --base)
    repo.references.create("refs/remotes/origin/main", oid)

    return repo


class TestAddWorktreeIntegration:
    def test_default_creates_worktree_under_tmp(self, git_repo: pygit2.Repository) -> None:
        result = runner.invoke(
            app,
            ["git", "add-worktree", git_repo.workdir.rstrip("/"), "feature-a"],
        )
        assert result.exit_code == 0, result.output
        wt_path = Path(result.output.strip())
        assert wt_path.exists()
        assert wt_path.name.startswith("sodagun-wt-repo-")

    def test_custom_dir_prefix(self, git_repo: pygit2.Repository, tmp_path: Path) -> None:
        prefix = tmp_path / "worktrees"
        prefix.mkdir()
        result = runner.invoke(
            app,
            [
                "git",
                "add-worktree",
                git_repo.workdir.rstrip("/"),
                "feature-b",
                "--dir-prefix",
                str(prefix),
            ],
        )
        assert result.exit_code == 0, result.output
        wt_path = Path(result.output.strip())
        assert wt_path.parent == prefix
        assert wt_path.name.startswith("sodagun-wt-repo-")
        assert wt_path.exists()

    def test_json_output(self, git_repo: pygit2.Repository) -> None:
        result = runner.invoke(
            app,
            [
                "--output",
                "json",
                "git",
                "add-worktree",
                git_repo.workdir.rstrip("/"),
                "feature-c",
            ],
        )
        assert result.exit_code == 0, result.output
        data = json.loads(result.output.strip())
        assert data["status"] == "ok"
        assert Path(data["worktree_path"]).exists()
