"""`--prepare`: ask the build system for the artifacts the chosen cells test, before anything is contacted.

Which `cargo xtask dist` target makes which artifact is the closed table
`TARGETS`: a (platform, delivery, architecture) and the file the target
writes to dist/ (tools/xtask/src/dist.rs). The architecture is read from the
configured artifact's name, so a cell can be prepared only when its
configuration names that file in dist/; otherwise preparing fails, as it
would test another file than the one built. A cell without a configured
artifact is not prepared (it is skipped, or fails when named, as without
`--prepare`).
"""

from collections.abc import Iterable
from enum import StrEnum
from pathlib import Path

from prysm_acceptance import commands
from prysm_acceptance.config import Config
from prysm_acceptance.options import Cell, Delivery, Platform


class Arch(StrEnum):
    """The architectures `cargo xtask dist` names, as Debian and Docker do."""

    ARM64 = 'arm64'
    AMD64 = 'amd64'


# (platform, delivery, architecture) -> (`cargo xtask dist` target, the file it writes in dist/).
TARGETS: dict[tuple[Platform, Delivery, Arch], tuple[str, str]] = {
    (Platform.MAC, Delivery.BINARY, Arch.ARM64): ('mac', 'prysm-headless-macos-arm64'),
    (Platform.MAC, Delivery.BINARY, Arch.AMD64): ('mac', 'prysm-headless-macos-amd64'),
    (Platform.LINUX, Delivery.BINARY, Arch.ARM64): ('linux-arm64', 'prysm-headless-linux-arm64'),
    (Platform.LINUX, Delivery.BINARY, Arch.AMD64): ('linux-amd64', 'prysm-headless-linux-amd64'),
    (Platform.LINUX, Delivery.DEB, Arch.ARM64): ('deb-arm64', 'prysm-headless-linux-arm64.deb'),
    (Platform.LINUX, Delivery.DEB, Arch.AMD64): ('deb-amd64', 'prysm-headless-linux-amd64.deb'),
    (Platform.LINUX, Delivery.IMAGE, Arch.ARM64): ('image-arm64', 'prysm-headless-linux-arm64.tar'),
    (Platform.LINUX, Delivery.IMAGE, Arch.AMD64): ('image-amd64', 'prysm-headless-linux-amd64.tar'),
}
DIST = commands.ROOT / 'dist'


class PrepareError(ValueError):
    """The configuration names an artifact the build system does not make."""


def target(platform: Platform, delivery: Delivery, artifact: Path) -> str:
    """The `cargo xtask dist` target that writes `artifact`."""
    for (where, how, _arch), (name, file) in TARGETS.items():
        if (where, how, artifact) == (platform, delivery, DIST / file):
            return name
    files = [file for (where, how, _arch), (_name, file) in TARGETS.items() if (where, how) == (platform, delivery)]
    raise PrepareError(
        f"[{platform}] {delivery} is {artifact}; --prepare builds only dist/ under the build system's names: "
        + ', '.join(f'dist/{file}' for file in files)
    )


def command(cells: Iterable[Cell], config: Config) -> commands.Command | None:
    """The one build command for the configured artifacts of `cells`; None when none is configured."""
    found: list[str] = []
    for cell in cells:
        artifact = config.machine(cell.platform).artifact(cell.delivery)
        if artifact is not None:
            name = target(cell.platform, cell.delivery, artifact)
            if name not in found:
                found.append(name)
    return commands.xtask_dist(found) if found else None
