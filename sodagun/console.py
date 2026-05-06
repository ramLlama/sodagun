"""Shared Rich console instances."""

from rich.console import Console

stdout = Console()
stderr = Console(stderr=True)
