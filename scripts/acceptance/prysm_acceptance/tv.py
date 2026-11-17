"""The TV through the allow-listed commands: reach and wake it, Luna state, install and uninstall, Prysm's
files, the worker's load, and Prysm as it may already be there (`installed.Existing`)."""

from collections.abc import Callable, Sequence
from enum import StrEnum
from itertools import pairwise
import json
import re
import time
from typing import NotRequired, TypedDict, cast

from prysm_acceptance import commands, installed, prysm_api, web
from prysm_acceptance.checks import Failed
from prysm_acceptance.commands import Command
from prysm_acceptance.installed import Backup
from prysm_acceptance.options import Flavour
from prysm_acceptance.prysm_api import Target
from prysm_acceptance.transport import Result, Transport

CLOCK_TICKS = 100  # USER_HZ on the TV's ARM kernel (assumed; see the report)
# How long a woken TV may take until it answers over SSH.
WAKE_SECONDS = 90


class Encoding(StrEnum):
    """How the slim package streams the picture."""

    H264_HARDWARE = 'h264_hardware'
    H264_SOFTWARE = 'h264_software'
    RAW = 'raw'


class Preferences(TypedDict, total=False):
    autostart: bool
    instance: str | None


class Worker(TypedDict):
    phase: str


class TvFailure(TypedDict, total=False):
    kind: str
    message: str


class LunaReply(TypedDict, total=False):
    """A Luna reply; `getState`'s fields (the service's state) as the checks read them."""

    returnValue: bool
    errorText: str
    flavor: Flavour
    preferences: Preferences
    enabled: bool
    lighting: Target | None
    power: str
    worker: Worker
    capturing: bool
    paused: bool
    fps: float | None
    portal: str | None
    failure: TvFailure | None
    encoding: str | None
    log: list[str]


class Reading(TypedDict):
    """`sample_worker`: one reading of the TV's uptime and load and the worker's CPU ticks and RSS."""

    uptime: float
    load: str
    ticks: int | None
    rss_kb: int | None


class Load(TypedDict):
    cpu_mean: float | None
    cpu_max: float | None
    rss_max_kb: int | None
    load: list[str]


class Soak(prysm_api.Rates, Load):
    """`Tv.soak`: the rates Prysm received and the TV worker's load over `seconds`."""

    seconds: float
    size: NotRequired[str]


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


def cpu_percent(first: Reading, second: Reading) -> float | None:
    """The worker's CPU between two readings, in percent of one core."""
    if first['ticks'] is None or second['ticks'] is None or second['uptime'] <= first['uptime']:
        return None
    return round(100 * (second['ticks'] - first['ticks']) / CLOCK_TICKS / (second['uptime'] - first['uptime']), 1)


def load_summary(readings: Sequence[Reading]) -> Load:
    cpu = [c for c in (cpu_percent(a, b) for a, b in pairwise(readings)) if c is not None]
    rss = [r['rss_kb'] for r in readings if r['rss_kb']]
    return {
        'cpu_mean': round(sum(cpu) / len(cpu), 1) if cpu else None,
        'cpu_max': max(cpu, default=None),
        'rss_max_kb': max(rss, default=None),
        'load': [r['load'] for r in readings][-1:],
    }


def soak_summary(result: Soak) -> str:
    return (
        f'{result["fps_mean"]} fps (min {result["fps_min"]}), worker CPU {result["cpu_mean"]} % '
        f'(max {result["cpu_max"]}), RSS {result["rss_max_kb"]} kB'
    )


class Tv:
    """The TV at `target` (root@HOST)."""

    name = 'tv'

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

    def get_state(self) -> LunaReply | None:
        """`getState`, or `None` when the service does not answer (not registered)."""
        try:
            reply = luna_reply(self._run(commands.tv_get_state(self.target), check=False))
        except Failed:
            return None
        return reply if reply.get('returnValue') else None

    def wait_state(self, check: Callable[[LunaReply], object], seconds: float, every: float = 2.0) -> LunaReply | None:
        deadline = time.monotonic() + seconds
        while True:
            state = self.get_state()
            if state is not None and check(state):
                return state
            if time.monotonic() >= deadline:
                return None
            time.sleep(every)

    def call(self, command: Command) -> LunaReply:
        reply = luna_reply(self._run(command, check=False))
        if not reply.get('returnValue'):
            raise Failed(f'{command.template} refused: {reply.get("errorText")}', reply)
        return reply

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
        state = self.wait_state(lambda s: True, 30)
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

    def uninstall(self) -> list[str]:
        """The supervisor's `uninstall`, then the app removed through the TV's install service; what of Prysm
        is still there (`installed`)."""
        self._run(commands.tv_supervisor_uninstall(self.target), check=False)
        removed = self._run(commands.tv_remove_app(self.target), check=False)
        time.sleep(3)
        left = self.installed()
        if left and not removed.ok:
            raise Failed('appInstallService did not report removed', (removed.stdout + removed.stderr)[-2000:])
        return left

    # ---- the worker's load -------------------------------------------------------

    def sample_worker(self) -> Reading:
        """One reading: uptime, load average, the worker's CPU ticks and RSS (None without a worker)."""
        lines = self._run(commands.tv_sample_worker(self.target)).stdout.splitlines()
        reading: Reading = {
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

    def soak(self, prysm: prysm_api.Prysm, seconds: float) -> Soak:
        """`prysm`'s input every second and the TV worker's load every 5 s, for `seconds`."""
        inputs: list[prysm_api.Input | None] = []
        readings: list[Reading] = []
        deadline = time.monotonic() + seconds
        next_reading = 0.0
        while time.monotonic() < deadline:
            if time.monotonic() >= next_reading:
                readings.append(self.sample_worker())
                next_reading = time.monotonic() + 5
            inputs.append(prysm.state().get('input'))
            time.sleep(1)
        readings.append(self.sample_worker())
        return {**prysm_api.rates(inputs), **load_summary(readings), 'seconds': seconds}

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
