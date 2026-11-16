"""The command-line options the stages read (`run.arguments` parses them)."""

from enum import StrEnum
from typing import Protocol


class Source(StrEnum):
    """Where the desktop stage's picture comes from: a clip (`file`, `file:PATH`) or a capture card."""

    FILE = 'file'
    DEVICE = 'device'


class Options(Protocol):
    """The options a stage's plan and steps depend on; read-only."""

    @property
    def source(self) -> Source: ...
    @property
    def source_path(self) -> str | None: ...
    @property
    def display_check(self) -> bool: ...
    @property
    def no_build(self) -> bool: ...
    @property
    def sweep(self) -> bool: ...
    @property
    def tv_is_free(self) -> bool: ...
