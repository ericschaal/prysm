"""`--prepare`: ask the build system for the artifacts the chosen cells test, before anything is contacted.

Which `cargo xtask dist` target makes which artifact is the closed table
`TARGETS`: for a configuration key, each file a target writes to dist/
(tools/xtask/src/dist.rs). A cell can be prepared only when its
configuration names one of those files in dist/; otherwise preparing fails,
as it would test another file than the one built. A cell without a
configured artifact is not prepared (it is skipped, or fails when named, as
without `--prepare`). A slim TV cell needs its package and its receiver's
binary.
"""

from collections.abc import Iterable
from pathlib import Path

from prysm_acceptance import commands
from prysm_acceptance.config import Config
from prysm_acceptance.options import Cell, TvCell

# The configuration key -> {the file in dist/: the `cargo xtask dist` target that writes it}.
TARGETS: dict[str, dict[str, str]] = {
    '[mac] binary': {'prysm-headless-macos-arm64': 'mac', 'prysm-headless-macos-amd64': 'mac'},
    '[linux] binary': {'prysm-headless-linux-arm64': 'linux-arm64', 'prysm-headless-linux-amd64': 'linux-amd64'},
    '[linux] deb': {'prysm-headless-linux-arm64.deb': 'deb-arm64', 'prysm-headless-linux-amd64.deb': 'deb-amd64'},
    '[linux] image': {'prysm-headless-linux-arm64.tar': 'image-arm64', 'prysm-headless-linux-amd64.tar': 'image-amd64'},
    '[tv] ipk_full': {'prysm-tv-full.ipk': 'tv-full'},
    '[tv] ipk_slim': {'prysm-tv-slim.ipk': 'tv-slim'},
}
DIST = commands.ROOT / 'dist'


class PrepareError(ValueError):
    """The configuration names an artifact the build system does not make."""


def target(key: str, artifact: Path) -> str:
    """The `cargo xtask dist` target that writes `artifact`, configured as `key`."""
    files = TARGETS[key]
    if artifact.parent == DIST and artifact.name in files:
        return files[artifact.name]
    raise PrepareError(
        f"{key} is {artifact}; --prepare builds only dist/ under the build system's names: "
        + ', '.join(f'dist/{file}' for file in files)
    )


def artifacts(cell: Cell | TvCell, config: Config) -> list[tuple[str, Path | None]]:
    """The artifacts `cell` tests, each with its configuration key."""
    if isinstance(cell, TvCell):
        package = (f'[tv] ipk_{cell.flavour}', config.tv.ipk(cell.flavour))
        if cell.receiver is None:
            return [package]
        return [package, (f'[{cell.receiver}] binary', config.machine(cell.receiver).artifact())]
    return [(f'[{cell.platform}] {cell.delivery}', config.machine(cell.platform).artifact(cell.delivery))]


def command(cells: Iterable[Cell | TvCell], config: Config) -> commands.Command | None:
    """The one build command for the configured artifacts of `cells`; None when none is configured."""
    found: list[str] = []
    for cell in cells:
        for key, artifact in artifacts(cell, config):
            if artifact is not None:
                name = target(key, artifact)
                if name not in found:
                    found.append(name)
    return commands.xtask_dist(found) if found else None
