"""Entry point for the sodagun CLI."""

import importlib.metadata
from typing import Annotated

import typer

from sodagun.commands.git import git_app
from sodagun.console import stdout
from sodagun.context import Context, OutputFormat

app = typer.Typer(help="sodagun CLI", no_args_is_help=True)
app.add_typer(git_app)


def _version_callback(value: bool) -> None:
    if value:
        v = importlib.metadata.version("sodagun")
        stdout.print(f"[bold]sodagun[/bold] {v}")
        raise typer.Exit()


@app.callback()
def main(
    ctx: typer.Context,
    version: Annotated[
        bool | None,
        typer.Option(
            "--version", callback=_version_callback, is_eager=True, help="Print version and exit."
        ),
    ] = None,
    output: Annotated[
        OutputFormat,
        typer.Option("--output", help="Output format."),
    ] = OutputFormat.TEXT,
) -> None:
    ctx.obj = Context(output=output)


if __name__ == "__main__":
    app()
