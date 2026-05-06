"""Entry point for the sodagun CLI."""

import importlib.metadata
from typing import Annotated

import typer
from rich.console import Console

app = typer.Typer(help="sodagun CLI", no_args_is_help=True)
console = Console()


def _version_callback(value: bool) -> None:
    if value:
        v = importlib.metadata.version("sodagun")
        console.print(f"[bold]sodagun[/bold] {v}")
        raise typer.Exit()


@app.callback()
def main(
    version: Annotated[
        bool | None,
        typer.Option(
            "--version", callback=_version_callback, is_eager=True, help="Print version and exit."
        ),
    ] = None,
) -> None:
    pass


if __name__ == "__main__":
    app()
