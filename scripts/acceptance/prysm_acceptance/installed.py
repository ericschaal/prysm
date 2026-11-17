"""An existing Prysm on a cell's target: none before a run, none after it (`installed = "refuse" | "replace"`).

The TV is the owner's daily device and the Linux package cell's target may
be a computer in use, so one rule holds for both. With `refuse` (the
default) a cell stops before changing anything and says what it found. With
`replace` it reads, through the running Prysm's own API, what a user would
lose (its settings and lighting target, and on the TV the preferences and
the data files), saves that as one backup file in the run directory, reads
the file back and compares it, and only then uninstalls Prysm. The cell
cleans up as always, so the target is left without Prysm: it is never
reinstalled. `warning` says so at the end of the run, also after a failure
or Ctrl-C, with the backup and the command that restores it into the Prysm
the user reinstalls (`acceptance settings restore TARGET BACKUP`).

`Existing` is what a target implements (`tv.Tv`, `host_deb.DebPrysm`); the
rule, the backup's file and the warning are written once, here.
"""

from collections.abc import Callable
import datetime
import json
from pathlib import Path
from typing import Protocol, TypedDict, cast

from prysm_acceptance.checks import Failed
from prysm_acceptance.prysm_api import Settings, Target
from prysm_acceptance.web import Json, JsonObject

BACKUP = 'prysm-backup.json'
RESTORE = 'uv run --no-dev --project scripts/acceptance acceptance settings restore'


class Portal(TypedDict):
    """What the portal API gave: where it answered, the settings and the lighting target as found."""

    url: str
    settings: Settings
    target: Target


class Backup(TypedDict):
    """The backup file (`BACKUP`): what `restore` sends back, and what it was taken from."""

    target: str  # the configured target's name: `tv` or `linux`
    host: str
    taken: str
    found: list[str]
    portal: Portal | None  # the full TV package's portal, or the Linux package's
    luna: JsonObject | None  # the TV service's getState: flavor, preferences, enabled, lighting
    files: dict[str, str | None]  # the TV's data files as they were (None: absent), raw


class Existing(Protocol):
    """Prysm as it may already be on a target."""

    @property
    def name(self) -> str:
        """The configured target: `tv` or `linux`."""
        ...

    def found(self) -> list[str]:
        """What of Prysm is on the target (paths, processes, a package, a service that answers); empty: none."""
        ...

    def backup(self, found: list[str]) -> Backup:
        """What a user would lose, read through the running Prysm's API; raises when it cannot be read."""
        ...

    def remove(self) -> list[str]:
        """Uninstall Prysm; what of it is still there (empty: nothing)."""
        ...

    def restore(self, backup: Backup) -> list[str]:
        """Send what `backup` holds to the Prysm reinstalled there, through its API; what was done. Raises when
        no Prysm answers. Lighting is not switched on."""
        ...


def refusal(existing: Existing, found: list[str]) -> str | None:
    """Why the cell stops with `installed = "refuse"`; None when nothing of Prysm was found."""
    if not found:
        return None
    return (
        f'Prysm is on the {existing.name} target: {", ".join(found)}. Nothing was changed. Remove it there, or '
        f'set [{existing.name}] installed = "replace" to back up its settings and uninstall it (it is not '
        'reinstalled)'
    )


def take(existing: Existing, found: list[str], path: Path) -> Backup:
    """The backup, written to `path`, read back and compared; Failed (nothing removed) when any of it fails."""
    try:
        backup = existing.backup(found)
    except (Failed, RuntimeError, OSError) as error:
        raise Failed(
            f'no backup of the Prysm on the {existing.name} target, so nothing was removed: {error}'
        ) from error
    path.write_text(json.dumps(backup, indent=2) + '\n')
    if json.loads(path.read_text()) != json.loads(json.dumps(backup)):
        raise Failed(f'{path} does not read back as written, so nothing was removed')
    return backup


def replace(existing: Existing, found: list[str], path: Path, warn: Callable[[str], None]) -> str:
    """`installed = "replace"`: the backup taken and verified, the end-of-run warning registered, then Prysm
    uninstalled; Failed when the backup fails (nothing removed) or the uninstall leaves anything."""
    backup = take(existing, found, path)
    installed = backup['luna'] is not None or backup['portal'] is not None
    warn(warning(existing, backup, path, removed=installed))
    if not installed:
        return f'only data files of Prysm were there; backed up in {path}'
    left = existing.remove()
    if left:
        raise Failed(f'the uninstall left on the {existing.name} target: {", ".join(left)}')
    return f'backed up in {path}, then uninstalled'


def stamp() -> str:
    return datetime.datetime.now(datetime.UTC).isoformat(timespec='seconds')


def warning(existing: Existing, backup: Backup, path: Path, removed: bool) -> str:
    """What the end of the run says after `replace`."""
    command = f'{RESTORE} {existing.name} {path}'
    if not removed:
        return (
            f'Prysm was not installed on the {existing.name} target ({backup["host"]}), but its data files were '
            f'there and this run changed them: as they were, they are in {path}. After installing Prysm there, '
            f"{command} sends them to it. If an earlier acceptance run replaced Prysm there, restore that run's "
            'backup instead: these files are what it left.'
        )
    return (
        f'Prysm was REMOVED from the {existing.name} target ({backup["host"]}) and NOT reinstalled. Reinstalling the '
        f'right version is your responsibility. Its settings are backed up in {path}; once Prysm is installed and '
        f'running there again, restore them with: {command}'
    )


def load(path: Path, name: str) -> Backup:
    """A backup file of the target `name`; ValueError when it is not one."""
    backup = cast('Backup', json.loads(path.read_text()))
    if not isinstance(cast('Json', backup), dict) or backup.get('target') != name:
        raise ValueError(f'{path} is not a backup of the {name} target')
    return backup
