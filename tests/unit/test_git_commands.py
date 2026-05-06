"""Unit tests for git subcommands (pygit2 mocked)."""

from __future__ import annotations

import json
import tempfile
from collections.abc import Iterator
from pathlib import Path
from unittest.mock import MagicMock, patch

import pygit2
import pytest
from typer.testing import CliRunner

from sodagun.cli import app

runner = CliRunner()

_BASE_ARGS = ["/fake/repo", "my-branch"]
_TMP = Path(tempfile.gettempdir())
_REPO_PATCH = "sodagun.commands.git.pygit2.Repository"


def _make_repo_mock(
    *,
    branch_side_effect: Exception | None = None,
    worktree_side_effect: Exception | None = None,
    revparse_side_effect: Exception | None = None,
    list_worktrees_return: list[str] | None = None,
) -> MagicMock:
    """Return a mock pygit2 Repository instance for happy-path or error injection."""
    repo = MagicMock()
    commit = MagicMock(spec=pygit2.Commit)

    if revparse_side_effect:
        repo.revparse_single.side_effect = revparse_side_effect
    else:
        obj = MagicMock()
        obj.peel.return_value = commit
        repo.revparse_single.return_value = obj

    if branch_side_effect:
        repo.branches.create.side_effect = branch_side_effect
    else:
        repo.branches.create.return_value = MagicMock()

    if worktree_side_effect:
        repo.add_worktree.side_effect = worktree_side_effect

    repo.list_worktrees.return_value = list_worktrees_return or []

    return repo


class TestAddWorktreeSuccess:
    @pytest.fixture(autouse=True)
    def mock_repo(self) -> Iterator[MagicMock]:
        """Patches pygit2.Repository; yields the constructor mock (instance is .return_value)."""
        with patch(_REPO_PATCH) as constructor:
            constructor.return_value = _make_repo_mock()
            yield constructor

    def test_text_output_is_sodagun_wt_path(self) -> None:
        result = runner.invoke(app, ["git", "add-worktree"] + _BASE_ARGS)
        assert result.exit_code == 0, result.output
        # path is <tmpdir>/sodagun-wt-<reponame>-<uuid8>; repo name is "repo"
        assert result.output.strip().startswith(str(_TMP / "sodagun-wt-repo-"))

    def test_json_output_ok(self) -> None:
        result = runner.invoke(app, ["--output", "json", "git", "add-worktree"] + _BASE_ARGS)
        assert result.exit_code == 0, result.output
        data = json.loads(result.output.strip())
        assert data["status"] == "ok"
        assert "worktree_path" in data

    def test_custom_dir_prefix_used(self, tmp_path: Path) -> None:
        result = runner.invoke(
            app,
            ["git", "add-worktree"] + _BASE_ARGS + ["--dir-prefix", str(tmp_path)],
        )
        assert result.exit_code == 0, result.output
        assert result.output.strip().startswith(str(tmp_path / "sodagun-wt-repo-"))

    def test_custom_base_passed_to_revparse(self, mock_repo: MagicMock) -> None:
        result = runner.invoke(
            app,
            ["git", "add-worktree"] + _BASE_ARGS + ["--base", "refs/heads/develop"],
        )
        assert result.exit_code == 0, result.output
        mock_repo.return_value.revparse_single.assert_called_once_with("refs/heads/develop")


class TestAddWorktreeErrors:
    @pytest.fixture(autouse=True)
    def mock_repo(self) -> Iterator[MagicMock]:
        """Patches pygit2.Repository; yields the constructor mock.

        Tests set .side_effect to simulate a failing constructor, or configure
        .return_value.<method>.side_effect for instance-level failures.
        """
        with patch(_REPO_PATCH) as constructor:
            constructor.return_value = _make_repo_mock()
            yield constructor

    def test_repo_not_found_text(self, mock_repo: MagicMock) -> None:
        mock_repo.side_effect = pygit2.GitError("not a repo")
        result = runner.invoke(app, ["git", "add-worktree"] + _BASE_ARGS)
        assert result.exit_code == 1
        assert "REPO_NOT_FOUND" in result.output

    def test_repo_not_found_json(self, mock_repo: MagicMock) -> None:
        mock_repo.side_effect = pygit2.GitError("not a repo")
        result = runner.invoke(app, ["--output", "json", "git", "add-worktree"] + _BASE_ARGS)
        assert result.exit_code == 1
        data = json.loads(result.output.strip())
        assert data == {"status": "error", "code": "REPO_NOT_FOUND"}

    def test_base_not_found(self, mock_repo: MagicMock) -> None:
        mock_repo.return_value.revparse_single.side_effect = KeyError("origin/main")
        result = runner.invoke(app, ["git", "add-worktree"] + _BASE_ARGS)
        assert result.exit_code == 1
        assert "BASE_NOT_FOUND" in result.output

    def test_base_not_found_json(self, mock_repo: MagicMock) -> None:
        mock_repo.return_value.revparse_single.side_effect = KeyError("origin/main")
        result = runner.invoke(app, ["--output", "json", "git", "add-worktree"] + _BASE_ARGS)
        assert result.exit_code == 1
        data = json.loads(result.output.strip())
        assert data == {"status": "error", "code": "BASE_NOT_FOUND"}

    def test_branch_already_exists_text(self, mock_repo: MagicMock) -> None:
        mock_repo.return_value.branches.create.side_effect = pygit2.AlreadyExistsError("exists")
        result = runner.invoke(app, ["git", "add-worktree"] + _BASE_ARGS)
        assert result.exit_code == 1
        assert "BRANCH_EXISTS" in result.output

    def test_branch_already_exists_json(self, mock_repo: MagicMock) -> None:
        mock_repo.return_value.branches.create.side_effect = pygit2.AlreadyExistsError("exists")
        result = runner.invoke(app, ["--output", "json", "git", "add-worktree"] + _BASE_ARGS)
        assert result.exit_code == 1
        data = json.loads(result.output.strip())
        assert data == {"status": "error", "code": "BRANCH_EXISTS"}

    def test_worktree_exists_via_list(self, mock_repo: MagicMock) -> None:
        mock_repo.return_value.list_worktrees.return_value = ["my-branch"]
        result = runner.invoke(app, ["git", "add-worktree"] + _BASE_ARGS)
        assert result.exit_code == 1
        assert "WORKTREE_EXISTS" in result.output
        mock_repo.return_value.branches.create.return_value.delete.assert_called_once()

    def test_generic_git_error_on_worktree_add(self, mock_repo: MagicMock) -> None:
        mock_repo.return_value.add_worktree.side_effect = pygit2.GitError("disk quota exceeded")
        result = runner.invoke(app, ["git", "add-worktree"] + _BASE_ARGS)
        assert result.exit_code == 1
        assert "GIT_ERROR" in result.output
        mock_repo.return_value.branches.create.return_value.delete.assert_called_once()
