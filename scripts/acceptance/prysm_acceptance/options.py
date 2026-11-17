"""The closed sets a run is chosen by (a host cell's platform, delivery and source) and the
command-line options the `tv` stage reads (`run.arguments` parses them)."""

from dataclasses import dataclass
from enum import StrEnum
from typing import Protocol


class Platform(StrEnum):
    """Where a host cell runs Prysm (`[mac]`, `[linux]`)."""

    MAC = 'mac'
    LINUX = 'linux'


class Delivery(StrEnum):
    """How Prysm reaches the platform (docs/developers/acceptance-plan.md, One host seam)."""

    BINARY = 'binary'


class Source(StrEnum):
    """Where a host cell's picture comes from: the generated clip (`file`) or a capture card."""

    FILE = 'file'
    DEVICE = 'device'


@dataclass(frozen=True)
class Cell:
    """One host cell: a pytest parameter, `mac-binary-file` and so on."""

    platform: Platform
    delivery: Delivery
    source: Source

    @property
    def id(self) -> str:
        return f'{self.platform}-{self.delivery}-{self.source}'


class Options(Protocol):
    """The options the `tv` and `eyes` stages' plans and steps depend on; read-only."""

    @property
    def sweep(self) -> bool: ...
    @property
    def tv_is_free(self) -> bool: ...
