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

`Existing` is what a target implements (`webos.WebosTv`, `host_deb.DebPrysm`); the
rule, the backup's file and the warning are written once, here.
"""

from collections.abc import Callable
import datetime
import json
from pathlib import Path
from typing import Protocol, TypedDict, cast

from prysm_acceptance.checks import Failed
from prysm_acceptance.prysm_api import Prysm, Settings, Target
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
    luna: JsonObject | None  # the TV service's getState: flavor, preferences, and full lighting or slim enabled
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
        """Uninstall Prysm; what of it is still there (empty: nothing). Raises `Kept` when Prysm could not be
        stopped, so it was left installed rather than removed while running."""
        ...

    def restore(self, backup: Backup) -> list[str]:
        """Send what `backup` holds to the Prysm reinstalled there, through its API; what was done. Raises when
        no Prysm answers. Lighting is switched on when the backup recorded it on (`lighting`)."""
        ...


class Kept(Failed):
    """Prysm still runs after its uninstall was asked to stop it, so it was left installed as it is: removing
    or replacing a running Prysm ends it abruptly (on the TV, a killed worker was once seen to leave the TV's
    capture locked until a reboot; later kills did not reproduce it)."""


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
    """`installed = "replace"`: the backup taken and verified, then Prysm uninstalled, the end-of-run warning
    registered also when the uninstall fails or is interrupted; Failed when the backup fails (nothing removed) or
    the uninstall leaves anything, `Kept` (and its own warning) when Prysm could not be stopped."""
    backup = take(existing, found, path)
    if backup['luna'] is None and backup['portal'] is None:
        warn(warning(existing, backup, path, removed=False))
        return f'only data files of Prysm were there; backed up in {path}'
    kept: Kept | None = None
    try:
        left = existing.remove()
    except Kept as error:
        kept = error
        raise
    finally:
        warn(kept_warning(existing, backup, path, kept) if kept else warning(existing, backup, path, removed=True))
    if left:
        raise Failed(f'the uninstall left on the {existing.name} target: {", ".join(left)}')
    return f'backed up in {path}, then uninstalled'


def lighting(prysm: Prysm, target: Target) -> str:
    """The lighting target the backup recorded, put back through `prysm`'s API: switched on when it was on, left
    as it is when it was off (the restored Prysm starts as its preferences say); what was done."""
    if target == Target.OFF:
        return f'lighting was {target}: left as it is'
    reply = prysm.set_target(target)
    if reply.status != 204:
        raise RuntimeError(f'PUT {prysm.base}/api/v1/target: HTTP {reply.status} {reply.body[:300]!r}')
    return f'lighting {target}, as backed up'


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


def kept_warning(existing: Existing, backup: Backup, path: Path, kept: Kept) -> str:
    """What the end of the run says when `replace` left a Prysm that could not be stopped installed."""
    return (
        f'Prysm on the {existing.name} target ({backup["host"]}) did not stop, so this run did NOT REMOVE it while '
        f'it runs, but it is no longer as it was: {kept}. Its settings are backed up in {path}. Check Prysm there '
        f'(restart the {existing.name} target if it is stuck), then remove or set it up again yourself.'
    )


def load(path: Path, name: str) -> Backup:
    """A backup file of the target `name`; ValueError when it is not one."""
    backup = cast('Backup', json.loads(path.read_text()))
    if not isinstance(cast('Json', backup), dict) or backup.get('target') != name:
        raise ValueError(f'{path} is not a backup of the {name} target')
    return backup
