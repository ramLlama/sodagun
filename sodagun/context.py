"""Shared CLI context types."""

from __future__ import annotations

import enum
from dataclasses import dataclass, field


class OutputFormat(enum.StrEnum):
    TEXT = "text"
    JSON = "json"


@dataclass
class Context:
    output: OutputFormat = field(default=OutputFormat.TEXT)
