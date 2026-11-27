"""The closed sets a run is chosen by: a host cell's platform, delivery and source, and a TV cell's package
and slim receiver."""

from dataclasses import dataclass
from enum import StrEnum

from prysm_acceptance.wire_generated import Flavour


class Platform(StrEnum):
    """Where a host cell runs Prysm (`[mac]`, `[linux]`); also where a slim TV cell's receiver runs."""

    MAC = 'mac'
    LINUX = 'linux'


class Delivery(StrEnum):
    """How Prysm reaches the platform (docs/developers/acceptance.md, The host seam)."""

    BINARY = 'binary'
    DEB = 'deb'
    IMAGE = 'image'


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


@dataclass(frozen=True)
class TvCell:
    """One TV cell: `tv-full`, or `tv-slim-mac` and `tv-slim-linux` with the receiver on that platform."""

    flavour: Flavour
    receiver: Platform | None = None

    @property
    def id(self) -> str:
        return '-'.join(('tv', self.flavour, *([self.receiver] if self.receiver else [])))


# Every host cell, in the order `acceptance all` runs them. The Debian package and the image have no
# `file` source (docs/developers/acceptance.md, host section).
CELLS = (
    Cell(Platform.MAC, Delivery.BINARY, Source.FILE),
    Cell(Platform.MAC, Delivery.BINARY, Source.DEVICE),
    Cell(Platform.LINUX, Delivery.BINARY, Source.FILE),
    Cell(Platform.LINUX, Delivery.BINARY, Source.DEVICE),
    Cell(Platform.LINUX, Delivery.DEB, Source.DEVICE),
    Cell(Platform.LINUX, Delivery.IMAGE, Source.DEVICE),
)
# Every TV cell, in the order `acceptance all --tv-is-free` runs them, after the host cells.
TV_CELLS = (
    TvCell(Flavour.FULL),
    TvCell(Flavour.SLIM, Platform.MAC),
    TvCell(Flavour.SLIM, Platform.LINUX),
)
