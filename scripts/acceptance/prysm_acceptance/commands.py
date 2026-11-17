"""Every command the acceptance runner may run: the allow-list.

This module is the only place that builds a command line, local or on the
TV. Each command comes from one template in `TEMPLATES`; its parameters are
typed and checked against fixed patterns before anything is quoted, and the
remote shell text is fixed apart from those checked values. Nothing in a
configuration file becomes shell text.

On the TV the runner only touches Prysm's own app, service, Luna manifests,
boot hook and data directory, through the documented install path
(com.webos.appInstallService, then the package's setup.sh) and the
supervisor's own `uninstall`. The one file outside them is the staged
package, `STAGING`, removed after each install. Nothing writes to a system
partition, and nothing deletes `DATA_DIR` (the owner's settings live there).

On the Chromecast that feeds the capture card (`[mac.feed.chromecast]`), ADB
only reads its state and Wi-Fi address, wakes it and reads or sets the one
keep-awake setting; there is no free-form `adb shell`.
"""

from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from enum import StrEnum
import json
from pathlib import Path
import re
import shlex

# The repository: scripts/acceptance/prysm_acceptance/commands.py, three levels up.
ROOT = Path(__file__).resolve().parents[3]

APP = 'io.prysm.tv'
SERVICE = 'io.prysm.tv.service'
APP_DIR = '/media/developer/apps/usr/palm/applications/io.prysm.tv'
SERVICE_DIR = '/media/developer/apps/usr/palm/services/io.prysm.tv.service'
MANIFESTS = (
    '/var/luna-service2/manifests.d/io.prysm.tv.supervisor.json',
    '/var/luna-service2/manifests.d/io.prysm.tv.worker.json',
)
HOOK = '/var/lib/webosbrew/init.d/io.prysm.tv'
DATA_DIR = '/var/lib/io.prysm.tv'
STAGING_DIR = '/media/developer/temp'
STAGING = STAGING_DIR + '/io.prysm.tv.ipk'
SETUP = SERVICE_DIR + '/setup.sh'
SUPERVISOR = SERVICE_DIR + '/prysm-webos-service'
WORKER = SERVICE_DIR + '/prysm-webos-worker'
# What an install leaves (data excepted) and an uninstall must remove.
INSTALLED_PATHS = (APP_DIR, SERVICE_DIR, *MANIFESTS, HOOK)
DATA_FILES = ('tv.json', 'settings.json', 'resume.json')
# Every TV path a template may name, and the ones it may change.
TV_READ_PATHS = (*INSTALLED_PATHS, DATA_DIR, STAGING, '/proc')
TV_WRITE_PATHS = (STAGING_DIR, STAGING)
INSTALL_TIMEOUT = 120
LUNA_TIMEOUT = 12


class Flavour(StrEnum):
    """The two TV packages."""

    FULL = 'full'
    SLIM = 'slim'


_HOST = re.compile(r'(?:[A-Za-z0-9_][A-Za-z0-9._-]*@)?[A-Za-z0-9][A-Za-z0-9._-]*')
_INSTANCE = re.compile(r'^https?://[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?/?$')
_DEVICE = re.compile(r'^[A-Za-z0-9_-]{1,64}$')
_LISTEN = re.compile(r'^[0-9.]+:[0-9]{1,5}$')
_ORIGIN = re.compile(r'^(null|https?://[A-Za-z0-9.-]+(:[0-9]{1,5})?)$')


class Refused(ValueError):
    """A parameter a template does not accept."""


@dataclass(frozen=True)
class Command:
    """One allow-listed command, ready to run.

    `until`/`fail` (regular expressions) end a held-stdin command early on a
    matching reply; `stdin_file` streams a local file to it."""

    template: str
    argv: tuple[str, ...]
    timeout: float
    remote: str | None = None
    hold_stdin: bool = False
    stdin_file: str | None = None
    until: str | None = None
    fail: str | None = None
    background: bool = False
    env: dict[str, str] = field(default_factory=dict[str, str])

    def display(self) -> str:
        text = shlex.join(self.argv).replace(str(ROOT) + '/', '').replace(str(ROOT), '.')
        if self.stdin_file:
            text += ' < ' + shlex.quote(self.stdin_file)
        if self.hold_stdin:
            text = f'(stdin held open, at most {self.timeout:.0f} s) ' + text
        return text


# The checks take `object`: a parameter of the wrong type is refused like a wrong value.


def _check(pattern: re.Pattern[str], value: object, name: str) -> str:
    if not isinstance(value, str) or not pattern.fullmatch(value):
        raise Refused(f'{name}: {value!r} is not accepted')
    return value


def _flag(value: object, name: str) -> bool:
    if not isinstance(value, bool):
        raise Refused(f'{name} must be true or false')
    return value


def _number(value: object, name: str, low: int, high: int) -> int:
    """A whole number from `low` to `high`; never a bool."""
    if not isinstance(value, int) or isinstance(value, bool) or not low <= value <= high:
        raise Refused(f'{name}: {value!r}')
    return value


def _local_file(path: str | Path, name: str) -> str:
    path = Path(path)
    if '\0' in str(path) or str(path).startswith('-'):
        raise Refused(f'{name}: {path!s} is not accepted')
    return str(path)


# ---- the TV ----------------------------------------------------------------


def ssh_target(host: object) -> str:
    """`root@TV` or an ssh alias; never an option."""
    return _check(_HOST, host, 'tv.ssh')


def _ssh(
    template: str,
    host: str,
    remote: str,
    timeout: float,
    *,
    hold_stdin: bool = False,
    stdin_file: str | None = None,
    until: str | None = None,
    fail: str | None = None,
) -> Command:
    argv = (
        'ssh',
        '-o',
        'BatchMode=yes',
        '-o',
        'ConnectTimeout=8',
        '-o',
        'ServerAliveInterval=5',
        '-o',
        'ServerAliveCountMax=3',
        ssh_target(host),
        remote,
    )
    return Command(
        template, argv, timeout, remote=remote, hold_stdin=hold_stdin, stdin_file=stdin_file, until=until, fail=fail
    )


def _luna(
    template: str, host: str, method: str, payload: dict[str, object], app: bool = False, timeout: int = LUNA_TIMEOUT
) -> Command:
    # This TV's luna-send answers only while its stdin is open; the form used by install-ipk.sh.
    identity = f' -a {APP}' if app else ''
    remote = (
        f'timeout {timeout} luna-send{identity} -n 1 '
        f'luna://{SERVICE}/{method} {shlex.quote(json.dumps(payload, separators=(",", ":")))}'
    )
    return _ssh(template, host, remote, timeout + 5, hold_stdin=True)


def tv_reachable(host: str) -> Command:
    return _ssh('tv.reachable', host, 'echo prysm-acceptance-ok', 15)


def tv_read_data(host: str, name: str) -> Command:
    if name not in DATA_FILES:
        raise Refused(f'data file {name!r} is not one of {DATA_FILES}')
    path = f'{DATA_DIR}/{name}'
    return _ssh('tv.read-data', host, f'if [ -f {path} ]; then cat {path}; else echo __absent__; fi', 15)


def tv_paths(host: str) -> Command:
    """Which of Prysm's paths (and the staged package) exist."""
    paths = ' '.join((*INSTALLED_PATHS, DATA_DIR, STAGING))
    remote = f'for p in {paths}; do if [ -e "$p" ]; then echo "present $p"; else echo "absent $p"; fi; done'
    return _ssh('tv.paths', host, remote, 15)


def tv_processes(host: str) -> Command:
    """Processes whose executable lies under a Prysm app or service path."""
    remote = (
        'for e in /proc/[0-9]*/exe; do l=$(readlink "$e" 2>/dev/null) || continue; '
        'case "$l" in */io.prysm.*) p=${e#/proc/}; echo "${p%/exe} $l";; esac; done'
    )
    return _ssh('tv.processes', host, remote, 15)


def tv_sample_worker(host: str) -> Command:
    """The installed worker's /proc counters and the TV's uptime and load."""
    remote = (
        'cat /proc/uptime; cat /proc/loadavg; '
        'for e in /proc/[0-9]*/exe; do '
        f'[ "$(readlink "$e" 2>/dev/null)" = {WORKER} ] || continue; d=${{e%/exe}}; '
        'echo "stat $(cat "$d/stat")"; grep VmRSS "$d/status"; done'
    )
    return _ssh('tv.sample-worker', host, remote, 15)


def tv_stage(host: str, ipk: str) -> Command:
    """Copy a Prysm package to the staging path (the only file written outside Prysm's directories).

    Any file name: `tv.install` stages only a package `verify_ipk` accepted."""
    return _ssh(
        'tv.stage', host, f'mkdir -p {STAGING_DIR} && cat > {STAGING}', 120, stdin_file=_local_file(ipk, 'package')
    )


def tv_install(host: str) -> Command:
    """Install the staged package through the TV's install service."""
    payload = json.dumps({'id': APP, 'ipkUrl': STAGING, 'subscribe': True}, separators=(',', ':'))
    remote = (
        f'timeout {INSTALL_TIMEOUT} luna-send -i luna://com.webos.appInstallService/dev/install {shlex.quote(payload)}'
    )
    return _ssh(
        'tv.install',
        host,
        remote,
        INSTALL_TIMEOUT + 10,
        hold_stdin=True,
        until=r'"installed"',
        fail=r'"returnValue":\s*false|failed|FAILED',
    )


def tv_unstage(host: str) -> Command:
    return _ssh('tv.unstage', host, f'rm -f {STAGING}', 15)


def tv_setup(host: str) -> Command:
    """The package's setup.sh: Luna registration, boot hook, rescan, activation."""
    return _ssh('tv.setup', host, SETUP, 90)


def tv_supervisor_uninstall(host: str) -> Command:
    """The supervisor's own uninstall: stops the worker, removes hook and manifests, rescans."""
    return _ssh('tv.supervisor-uninstall', host, f'{SUPERVISOR} uninstall', 90)


def tv_get_state(host: str) -> Command:
    return _luna('tv.get-state', host, 'getState', {})


def tv_set_enabled(host: str, enabled: bool) -> Command:
    return _luna('tv.set-enabled', host, 'setEnabled', {'enabled': _flag(enabled, 'enabled')}, app=True)


def tv_set_preferences(host: str, autostart: bool, instance: str | None = None) -> Command:
    """Replace the preferences (the request is complete: autostart and, slim, instance)."""
    payload: dict[str, object] = {'autostart': _flag(autostart, 'autostart')}
    if instance is not None:
        payload['instance'] = _check(_INSTANCE, instance, 'instance')
    return _luna('tv.set-preferences', host, 'setPreferences', payload, app=True)


# ---- this computer ---------------------------------------------------------


def ares_remove(device: str) -> Command:
    """Remove the app with the webOS CLI (`ares-remove`), as tested on the TV."""
    return Command(
        'local.ares-remove', ('ares-remove', '--device', _check(_DEVICE, device, 'tv.ares_device'), APP), 120
    )


def verify_ipk(ipk: str) -> Command:
    """`ipk.py verify`: the package installs nothing outside the app and service directories."""
    return Command(
        'local.verify-ipk',
        ('python3', str(ROOT / 'packaging/webos/ipk.py'), 'verify', _local_file(ipk, 'package')),
        120,
    )


def git_last_commit_time() -> Command:
    """When the checkout's last commit was made (ISO 8601), to warn about an older artifact."""
    return Command('local.git-last-commit-time', ('git', '-C', str(ROOT), 'log', '-1', '--format=%cI'), 10)


def headless(binary: str, settings: str, listen: str, metrics: bool = True, allow_null_origin: bool = False) -> Command:
    """prysm-headless on `listen`, until stopped (SIGTERM)."""
    argv = [
        _local_file(binary, 'headless'),
        _local_file(settings, 'settings'),
        '--listen',
        _check(_LISTEN, listen, 'listen'),
    ]
    if metrics:
        argv.append('--metrics')
    if allow_null_origin:
        argv += ['--allow-origin', _check(_ORIGIN, 'null', 'origin')]
    return Command('local.headless', tuple(argv), 0, background=True, env={'RUST_LOG': 'warn,prysm=info'})


def ffmpeg_edges_clip(
    out: str, width: int, height: int, fps: int, seconds: int, corners: Sequence[str], step_seconds: int
) -> Command:
    """A clip of coloured edge bands over gray whose colours step round the corners.

    `corners`: four hex colours, clockwise from the top-left corner, at the
    first step. Each corner's colour fills the halves of the two edge bands
    that meet there. Every `step_seconds` the colours move one corner on,
    clockwise. `seconds` is a whole number of periods (four steps), so a
    player that repeats the clip keeps the order."""
    for value, name in (
        (width, 'width'),
        (height, 'height'),
        (fps, 'fps'),
        (seconds, 'seconds'),
        (step_seconds, 'step_seconds'),
    ):
        _number(value, name, 1, 4000)
    period = 4 * step_seconds
    if seconds % period:
        raise Refused(f'seconds: {seconds} is not a whole number of {period} s periods')
    if not isinstance(corners, (list, tuple)) or len(corners) != 4:
        raise Refused(f'corners: {corners!r}')
    for colour in corners:
        _check(re.compile(r'[0-9a-f]{6}'), colour, 'colour')
    band_w, band_h, half_w, half_h = width // 5, height // 5, width // 2, height // 2
    # (x, y, w, h) of the two half-bands at each corner, clockwise from the top-left.
    halves = [
        ((0, 0, half_w, band_h), (0, 0, band_w, half_h)),
        ((half_w, 0, width - half_w, band_h), (width - band_w, 0, band_w, half_h)),
        ((half_w, height - band_h, width - half_w, band_h), (width - band_w, half_h, band_w, height - half_h)),
        ((0, height - band_h, half_w, band_h), (0, half_h, band_w, height - half_h)),
    ]
    boxes = [
        f'drawbox=x={x}:y={y}:w={w}:h={h}:color=0x{corners[(corner - step) % 4]}:t=fill'
        f":enable='eq(floor(n/{step_seconds * fps}),{step})'"
        for step in range(4)
        for corner, parts in enumerate(halves)
        for x, y, w, h in parts
    ]
    # One period is drawn, then repeated frame for frame.
    source = (
        f'color=c=0x404040:s={width}x{height}:r={fps}:d={period},'
        + ','.join(boxes)
        + f',loop=loop={seconds // period - 1}:size={period * fps}'
    )
    # faststart: the index first, so a player streaming it over HTTP can start at once.
    return Command(
        'local.ffmpeg-clip',
        (
            'ffmpeg',
            '-nostdin',
            '-v',
            'error',
            '-y',
            '-f',
            'lavfi',
            '-i',
            source,
            '-c:v',
            'libx264',
            '-pix_fmt',
            'yuv420p',
            '-movflags',
            '+faststart',
            _local_file(out, 'clip'),
        ),
        300,
    )


# ---- the Chromecast's ADB (USB, or wireless debugging connected by hand) ----

_ADB_SERIAL = re.compile(r'[A-Za-z0-9]{4,32}|[A-Za-z0-9][A-Za-z0-9.-]*:[0-9]{1,5}')
# stay_on_while_plugged_in is a bit mask: 1 AC, 2 USB, 4 wireless charging, 8 dock.
STAY_ON = 7


def adb_serial(serial: object) -> str:
    """A USB serial or HOST:PORT; never an option."""
    return _check(_ADB_SERIAL, serial, 'feed.chromecast.adb_serial')


def _adb(template: str, serial: str, *words: str, timeout: float = 15) -> Command:
    return Command(template, ('adb', '-s', adb_serial(serial), *words), timeout)


def adb_get_state(serial: str) -> Command:
    """`device` when ADB reaches the Chromecast."""
    return _adb('adb.get-state', serial, 'get-state')


def adb_wake(serial: str) -> Command:
    return _adb('adb.wake', serial, 'shell', 'input keyevent KEYCODE_WAKEUP')


def adb_wakefulness(serial: str) -> Command:
    """The power manager's state, `mWakefulness=Awake` when the screen is on (read-only)."""
    return _adb('adb.wakefulness', serial, 'shell', 'dumpsys power | grep mWakefulness')


def adb_wifi_address(serial: str) -> Command:
    """The Wi-Fi interface's IPv4 address line (read-only)."""
    return _adb('adb.wifi-address', serial, 'shell', 'ip -f inet -o addr show wlan0')


def adb_stay_on_get(serial: str) -> Command:
    return _adb('adb.stay-on-get', serial, 'shell', 'settings get global stay_on_while_plugged_in')


def adb_stay_on_put(serial: str, value: int) -> Command:
    """Set the keep-awake mask: `STAY_ON`, or the value read before (0 to 15)."""
    _number(value, 'stay_on_while_plugged_in', 0, 15)
    return _adb('adb.stay-on-put', serial, 'shell', f'settings put global stay_on_while_plugged_in {value}')


# The allow-list: every template, with sample parameters for the tests and
# the dry-run listing.
TEMPLATES: dict[str, Callable[[], Command]] = {
    'tv.reachable': lambda: tv_reachable('root@tv.example'),
    'tv.read-data': lambda: tv_read_data('root@tv.example', 'settings.json'),
    'tv.paths': lambda: tv_paths('root@tv.example'),
    'tv.processes': lambda: tv_processes('root@tv.example'),
    'tv.sample-worker': lambda: tv_sample_worker('root@tv.example'),
    'tv.stage': lambda: tv_stage('root@tv.example', 'dist/prysm-tv-full.ipk'),
    'tv.install': lambda: tv_install('root@tv.example'),
    'tv.unstage': lambda: tv_unstage('root@tv.example'),
    'tv.setup': lambda: tv_setup('root@tv.example'),
    'tv.supervisor-uninstall': lambda: tv_supervisor_uninstall('root@tv.example'),
    'tv.get-state': lambda: tv_get_state('root@tv.example'),
    'tv.set-enabled': lambda: tv_set_enabled('root@tv.example', True),
    'tv.set-preferences': lambda: tv_set_preferences('root@tv.example', False, 'http://192.0.2.20:8787'),
    'local.ares-remove': lambda: ares_remove('tv'),
    'local.verify-ipk': lambda: verify_ipk('dist/prysm-tv-full.ipk'),
    'local.git-last-commit-time': git_last_commit_time,
    'local.headless': lambda: headless('dist/prysm-headless-macos-arm64', 'settings.json', '127.0.0.1:8787'),
    'local.ffmpeg-clip': lambda: ffmpeg_edges_clip(
        'clip.mp4', 640, 360, 30, 64, ('ff0000', '00ff00', '0000ff', 'ffff00'), 2
    ),
    'adb.get-state': lambda: adb_get_state('0123456789ABCD'),
    'adb.wake': lambda: adb_wake('0123456789ABCD'),
    'adb.wakefulness': lambda: adb_wakefulness('0123456789ABCD'),
    'adb.wifi-address': lambda: adb_wifi_address('0123456789ABCD'),
    'adb.stay-on-get': lambda: adb_stay_on_get('0123456789ABCD'),
    'adb.stay-on-put': lambda: adb_stay_on_put('192.0.2.60:37000', STAY_ON),
}
