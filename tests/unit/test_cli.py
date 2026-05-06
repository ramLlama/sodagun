"""Unit tests for the CLI."""

from typer.testing import CliRunner

from sodagun.cli import app

runner = CliRunner()


def test_version_flag() -> None:
    result = runner.invoke(app, ["--version"])
    assert result.exit_code == 0
    assert "sodagun" in result.output


def test_no_args_prints_help() -> None:
    result = runner.invoke(app, [])
    assert result.exit_code == 2  # no_args_is_help exits with 2
    assert "Usage" in result.output
