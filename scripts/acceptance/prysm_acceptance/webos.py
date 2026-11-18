"""The LG webOS TV through the allow-listed commands (`WebosTv`): reach and wake it, Luna state, install and
uninstall, Prysm's files, the worker's load (`tv.Tv`), and Prysm as it may already be there
(`installed.Existing`)."""

from collections.abc import Callable
import json
import re
import time
from typing import TypedDict, cast

from prysm_acceptance import commands, installed, prysm_api, tv, web
from prysm_acceptance.checks import Failed
from prysm_acceptance.commands import Command
from prysm_acceptance.installed import Backup
from prysm_acceptance.options import Flavour
from prysm_acceptance.prysm_api import Target
from prysm_acceptance.transport import Result, Transport

# How long a woken TV may take until it answers over SSH.
WAKE_SECONDS = 90
# The TV's Cast receiver, which plays the clip.
CAST_APP = 'com.webos.chromecast'
# How long Prysm's processes may take to go after the Luna `uninstall`: the worker's 8 s TERM-to-KILL window,
# then the supervisor's rescan (bounded at 10 s) before it exits.
STOP_SECONDS = 30


class Preferences(TypedDict, total=False):
    autostart: bool
    instance: str | None


class LunaReply(tv.State, total=False):
    """A Luna reply; `getState`'s fields (the service's state) as the checks read them."""

    returnValue: bool  # noqa: N815 -- Luna's field name (Ruff exempts it only on a direct TypedDict)
    errorText: str  # noqa: N815 -- Luna's field name
    flavor: Flavour
    preferences: Preferences
    enabled: bool
    lighting: Target | None
    portal: str | None


class Foreground(TypedDict, total=False):
    """The application manager's `getForegroundAppInfo` reply, as the runner reads it."""

    appId: str


def luna_reply(result: Result) -> LunaReply:
    """The first JSON object luna-send printed."""
    for line in result.stdout.splitlines():
        text = line.strip()
        if text.startswith('{'):
            try:
                return cast('LunaReply', json.loads(text))
            except ValueError:
                continue
    raise Failed('no JSON reply from luna-send', (result.stdout + result.stderr)[-1000:])


class WebosTv:
    """The TV at `target` (root@HOST)."""

    name = 'tv'
    prysm_app = commands.APP
    cast_app = CAST_APP

    def __init__(self, transport: Transport, target: str) -> None:
        self.transport = transport
        self.target = commands.ssh_target(target)

    def _run(self, command: Command, check: bool = True) -> Result:
        return self.transport.run(command, check=check)

    # ---- reaching it ---------------------------------------------------------

    def reachable(self) -> bool:
        return self._run(commands.tv_reachable(self.target), check=False).ok

    def reach(self, mac: str, towards: Callable[[], str]) -> str | None:
        """Why the TV cannot be reached over SSH; None when it answers. Only when it does not and `mac` is set,
        one wake-on-LAN packet is sent first, from `towards()` (this computer's address towards the TV), and SSH
        is tried every 5 s for `WAKE_SECONDS`. The runner never puts the TV to sleep."""
        if self.reachable():
            return None
        if not mac:
            return f'the TV ({self.target}) does not answer over SSH'
        try:
            sent = self._run(commands.wake_on_lan(mac, towards()), check=False)
        except OSError as error:  # no route to the TV from here
            return (
                f'the TV ({self.target}) does not answer over SSH, and no route to it for a wake-on-LAN packet: {error}'
            )
        if not sent.ok:
            return f'the TV ({self.target}) does not answer over SSH, and the wake-on-LAN packet failed: {sent.stderr}'
        deadline = time.monotonic() + WAKE_SECONDS
        while time.monotonic() < deadline:
            time.sleep(5)
            if self.reachable():
                return None
        return f'the TV ({self.target}) does not answer over SSH {WAKE_SECONDS} s after a wake-on-LAN packet'

    # ---- the service ---------------------------------------------------------

    def state_command(self) -> Command:
        return commands.tv_get_state(self.target)

    def get_state(self) -> LunaReply | None:
        """`getState`, or `None` when the service does not answer (not registered)."""
        try:
            reply = luna_reply(self._run(self.state_command(), check=False))
        except Failed:
            return None
        return reply if reply.get('returnValue') else None

    def call(self, command: Command) -> LunaReply:
        reply = luna_reply(self._run(command, check=False))
        if not reply.get('returnValue'):
            raise Failed(f'{command.template} refused: {reply.get("errorText")}', reply)
        return reply

    def foreground_command(self) -> Command:
        return commands.tv_foreground(self.target)

    def foreground(self) -> str:
        """The app in front; empty when the TV names none (also for a moment while one app replaces another)."""
        try:
            return cast('Foreground', self.call(self.foreground_command())).get('appId') or ''
        except Failed:
            return ''

    def launch_command(self) -> Command:
        return commands.tv_launch_app(self.target)

    def launch_app(self) -> None:
        """Prysm's own app in front."""
        self.call(self.launch_command())

    def launch_home(self) -> None:
        """The TV's home screen in front."""
        self.call(commands.tv_launch_home(self.target))

    def close_app(self) -> None:
        """Prysm's own app closed."""
        self.call(commands.tv_close_app(self.target))

    def preferences_command(self, preferences: Preferences) -> Command:
        """`setPreferences` with the whole of `preferences` (as the app)."""
        return commands.tv_set_preferences(
            self.target, bool(preferences.get('autostart')), preferences.get('instance') or None
        )

    def set_preferences(self, preferences: Preferences) -> None:
        self.call(self.preferences_command(preferences))

    def portal(self) -> prysm_api.Prysm:
        """The full package's portal, at the address `getState` reports once the worker runs."""
        url = (self.get_state() or LunaReply()).get('portal')
        if not url:
            raise Failed('getState reports no portal address: is the full worker running?')
        return prysm_api.Prysm(url)

    def wait_portal(self, seconds: float = 40) -> prysm_api.Prysm:
        """The full package's portal once it answers."""
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                prysm = self.portal()
                prysm.state()
            except (Failed, web.Unreachable, RuntimeError):
                time.sleep(1)
            else:
                return prysm
        raise Failed(f'the TV portal (getState portal) did not answer within {seconds} s')

    # ---- files and processes ---------------------------------------------------

    def paths(self) -> dict[str, bool]:
        found: dict[str, bool] = {}
        for line in self._run(commands.tv_paths(self.target)).stdout.splitlines():
            kind, _, path = line.partition(' ')
            if kind in ('present', 'absent'):
                found[path] = kind == 'present'
        return found

    def processes(self) -> list[list[str]]:
        return [
            line.split(' ', 1)
            for line in self._run(commands.tv_processes(self.target)).stdout.splitlines()
            if line.strip()
        ]

    def read_data(self, name: str) -> str | None:
        text = self._run(commands.tv_read_data(self.target, name)).stdout
        return None if text.strip() == '__absent__' else text

    # ---- install and uninstall -------------------------------------------------

    def install(self, ipk: str) -> LunaReply:
        """The documented install: verify, stage, appInstallService, unstage, setup.sh.

        `ipk.py verify` refuses a package with files outside the app and service
        directories; only a package it accepted is staged, whatever its name."""
        self._run(commands.verify_ipk(ipk))
        self._run(commands.tv_stage(self.target, ipk))
        try:
            result = self._run(commands.tv_install(self.target), check=False)
        finally:
            self._run(commands.tv_unstage(self.target), check=False)
        if not result.ok:
            raise Failed('appInstallService did not report installed', (result.stdout + result.stderr)[-2000:])
        self._run(commands.tv_setup(self.target))
        state = tv.wait_state(self.get_state, lambda s: True, 30)
        if state is None:
            raise Failed('the service does not answer getState 30 s after setup')
        return state

    def installed(self) -> list[str]:
        """What an install leaves and an uninstall must remove: the app, service, manifests and boot hook, the
        staged package, Prysm's processes, its Luna service answering. Not `commands.DATA_DIR`, which stays."""
        present = self.paths()
        found = [path for path in (*commands.INSTALLED_PATHS, commands.STAGING) if present.get(path)]
        found += [f'process {pid} {path}' for pid, path in self.processes()]
        state = self.get_state()
        if state is not None:
            found.append(f'the Luna service {commands.SERVICE} ({state.get("flavor")})')
        return found

    def running(self) -> list[list[str]]:
        """Prysm's processes (`processes`) once none is left, waiting up to `STOP_SECONDS`; those still there."""
        deadline = time.monotonic() + STOP_SECONDS
        while (left := self.processes()) and time.monotonic() < deadline:
            time.sleep(2)
        return left

    def uninstall(self) -> list[str]:
        """Prysm stopped through its Luna `uninstall` (the worker stops gracefully, then the supervisor
        deregisters and exits), no Prysm process left, the supervisor's `uninstall` (deregisters what is still
        registered), then the app removed through the TV's install service; what of Prysm is still there
        (`installed`). A refused or unanswered Luna `uninstall` is waited for the same way.

        Raises `installed.Kept`, the app left installed, while a Prysm process still runs: the install
        service would end it abruptly, and a worker killed with the TV UI capture open leaves the TV's
        capture locked until a reboot."""
        stopping = self._run(commands.tv_uninstall(self.target), check=False)
        left = self.running()
        if left:
            raise installed.Kept(
                f'Prysm still runs {STOP_SECONDS} s after its Luna uninstall '
                f'({", ".join(" ".join(process) for process in left)}), so the app was not removed',
                (stopping.stdout + stopping.stderr)[-2000:],
            )
        self._run(commands.tv_supervisor_uninstall(self.target), check=False)
        removed = self._run(commands.tv_remove_app(self.target), check=False)
        time.sleep(3)
        left = self.installed()
        if left and not removed.ok:
            raise Failed('appInstallService did not report removed', (removed.stdout + removed.stderr)[-2000:])
        return left

    # ---- the worker's load -------------------------------------------------------

    def sample_command(self) -> Command:
        return commands.tv_sample_worker(self.target)

    def sample_worker(self) -> tv.Reading:
        """One reading: uptime, load average, the worker's CPU ticks and RSS (None without a worker)."""
        lines = self._run(self.sample_command()).stdout.splitlines()
        reading: tv.Reading = {
            'uptime': float(lines[0].split()[0]),
            'load': lines[1].split()[0],
            'ticks': None,
            'rss_kb': None,
        }
        for line in lines[2:]:
            if line.startswith('stat '):
                fields = line.rsplit(')', 1)[1].split()
                reading['ticks'] = int(fields[11]) + int(fields[12])  # utime + stime
            match = re.match(r'VmRSS:\s+(\d+)', line)
            if match:
                reading['rss_kb'] = int(match[1])
        return reading

    # ---- Prysm as it may already be there (installed.Existing) ------------------

    def found(self) -> list[str]:
        """What is `installed`, and Prysm's data files (an earlier uninstall leaves them)."""
        found = self.installed()
        return found + [
            f'{commands.DATA_DIR}/{name}' for name in commands.DATA_FILES if self.read_data(name) is not None
        ]

    def backup(self, found: list[str]) -> Backup:
        """getState (flavour, preferences, enabled, lighting), the full portal's settings and target, and the
        data files as they are. An installed Prysm whose service or portal does not answer cannot be backed up."""
        files = {name: self.read_data(name) for name in commands.DATA_FILES}
        state = self.get_state()
        data_only = all(item.startswith(commands.DATA_DIR + '/') for item in found)
        if state is None and not data_only:
            raise RuntimeError(f'Prysm is installed but the Luna service {commands.SERVICE} does not answer getState')
        portal: installed.Portal | None = None
        if state is not None and state.get('flavor') == Flavour.FULL:
            prysm = self.wait_portal()
            answered = prysm.state()
            portal = {'url': prysm.base, 'settings': answered['settings'], 'target': answered['target']}
        luna = (
            None
            if state is None
            else {key: cast('web.Json', state.get(key)) for key in ('flavor', 'preferences', 'enabled', 'lighting')}
        )
        return {
            'target': self.name,
            'host': self.target,
            'taken': installed.stamp(),
            'found': found,
            'portal': portal,
            'luna': luna,
            'files': files,
        }

    def remove(self) -> list[str]:
        return self.uninstall()

    def restore(self, backup: Backup) -> list[str]:
        """The preferences, and for the full package the portal's settings, into the Prysm reinstalled here; from
        the files when the backup holds only them. Refuses when nothing answers or the package is the other
        flavour. Lighting and `enabled` are left as they are."""
        state = self.get_state()
        if state is None:
            raise RuntimeError(f'no Prysm answers on {self.target} (the Luna service {commands.SERVICE} getState)')
        luna = backup['luna']
        flavour = luna.get('flavor') if luna else state.get('flavor')
        if flavour != state.get('flavor'):
            raise RuntimeError(f'the backup is of the {flavour} package; {self.target} has {state.get("flavor")}')
        preferences = backed_up_preferences(backup)
        done: list[str] = []
        if preferences is not None:
            self.set_preferences(preferences)
            done.append(f'preferences {json.dumps(preferences)}')
        settings = backup['portal']['settings'] if backup['portal'] else _file_json(backup, 'settings.json')
        if flavour == Flavour.FULL and settings is not None:
            prysm = self.wait_portal()
            reply = prysm.replace_settings(cast('prysm_api.Settings', settings))
            if reply.status != 200:
                raise RuntimeError(f'PUT {prysm.base}/api/v1/settings: HTTP {reply.status} {reply.body[:300]!r}')
            done.append(f'settings into {prysm.base}')
        lighting = (luna or {}).get('lighting')
        done.append(f'lighting was {lighting or "unknown"}: not switched on; use the TV app')
        return done


def _file_json(backup: Backup, name: str) -> web.Json:
    text = backup['files'].get(name)
    return cast('web.Json', json.loads(text)) if text else None


def backed_up_preferences(backup: Backup) -> Preferences | None:
    """The preferences getState gave, or `tv.json` as it was (it holds them as setPreferences takes them)."""
    luna = backup['luna']
    found = luna.get('preferences') if luna else _file_json(backup, 'tv.json')
    return cast('Preferences', found) if isinstance(found, dict) else None
