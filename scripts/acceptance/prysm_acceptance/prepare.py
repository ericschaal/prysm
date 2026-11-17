"""`--prepare`: ask the build system for the artifacts the chosen cells test, before anything is contacted.

Which `cargo xtask dist` target makes which artifact is the closed table
`TARGETS`: for a platform and delivery, each file a target writes to dist/
(tools/xtask/src/dist.rs). A cell can be prepared only when its
configuration names one of those files in dist/; otherwise preparing fails,
as it would test another file than the one built. A cell without a
configured artifact is not prepared (it is skipped, or fails when named, as
without `--prepare`).
"""

from collections.abc import Iterable
from pathlib import Path

from prysm_acceptance import commands
from prysm_acceptance.config import Config
from prysm_acceptance.options import Cell, Delivery, Platform

# (platform, delivery) -> {the file in dist/: the `cargo xtask dist` target that writes it}.
TARGETS: dict[tuple[Platform, Delivery], dict[str, str]] = {
    (Platform.MAC, Delivery.BINARY): {'prysm-headless-macos-arm64': 'mac', 'prysm-headless-macos-amd64': 'mac'},
    (Platform.LINUX, Delivery.BINARY): {
        'prysm-headless-linux-arm64': 'linux-arm64',
        'prysm-headless-linux-amd64': 'linux-amd64',
    },
    (Platform.LINUX, Delivery.DEB): {
        'prysm-headless-linux-arm64.deb': 'deb-arm64',
        'prysm-headless-linux-amd64.deb': 'deb-amd64',
    },
    (Platform.LINUX, Delivery.IMAGE): {
        'prysm-headless-linux-arm64.tar': 'image-arm64',
        'prysm-headless-linux-amd64.tar': 'image-amd64',
    },
}
DIST = commands.ROOT / 'dist'


class PrepareError(ValueError):
    """The configuration names an artifact the build system does not make."""


def target(platform: Platform, delivery: Delivery, artifact: Path) -> str:
    """The `cargo xtask dist` target that writes `artifact`."""
    files = TARGETS[(platform, delivery)]
    if artifact.parent == DIST and artifact.name in files:
        return files[artifact.name]
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
