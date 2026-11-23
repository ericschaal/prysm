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
Beyond Prysm's own service, Luna is asked only which app is in front, and to
launch or close Prysm's own app (`APP`) and bring the home screen to the front
(`HOME_APP`), each id fixed in its template.
When the TV does not answer and `[tv] mac` is set, the runner sends it one
wake-on-LAN packet (`local.wake-on-lan`, a UDP broadcast, no process); it
never puts the TV to sleep.

On a Linux target over SSH (`[linux] target = "ssh://USER@HOST"`) the runner
writes only in the run's own directory, `$HOME/.prysm-acceptance/RUN` (the
templates take the checked run name, never a path), reads the kernel log,
the route to WLED and its own Prysm's state, signals only the process whose
executable is the binary it copied there, and removes that directory at the
end. The `linux.*` templates never use sudo.

On a Linux target and on the TV, `device.colours` reads Prysm's colours
stream on the device itself (python3 on its loopback, bounded by its own
`timeout`), so the colour check times the events by that device's clock; it
writes nothing.

The Debian package cell (`deb.*`) also installs the package with apt-get,
writes a runtime drop-in `DEB_DROPIN` under /run (gone at the next boot),
starts and stops the unit, reads its journal and purges the package at the
end (Prysm itself writes its settings, `DEB_SETTINGS`, through its portal);
only these templates use sudo, each spelling `sudo -n` itself. The image cell (`image.*`) runs plain
`docker` and `docker compose` (the ssh user is in the `docker` group) on the
image tag `IMAGE_TAG` and one compose project named after the run, and
removes both at the end.

On the Chromecast that feeds the capture card (`[mac.feed.chromecast]`), ADB
only reads its state and Wi-Fi address, wakes it and reads or sets the one
keep-awake setting; there is no free-form `adb shell`.
"""

from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from enum import StrEnum
import ipaddress
import json
import math
from pathlib import Path
import re
import shlex
from typing import TypeVar

# The repository: scripts/acceptance/prysm_acceptance/commands.py, three levels up.
ROOT = Path(__file__).resolve().parents[3]

APP = 'io.github.ericschaal.prysm'
SERVICE = f'{APP}.service'
# The earlier capturer, which must be removed before a run.
CAPTURE_APP = 'io.prysm.capture'
APP_MANAGER = 'com.webos.service.applicationManager'
HOME_APP = 'com.webos.app.home'
APP_DIR = f'/media/developer/apps/usr/palm/applications/{APP}'
SERVICE_DIR = f'/media/developer/apps/usr/palm/services/{SERVICE}'
MANIFESTS = (
    f'/var/luna-service2/manifests.d/{APP}.supervisor.json',
    f'/var/luna-service2/manifests.d/{APP}.worker.json',
)
HOOK = f'/var/lib/webosbrew/init.d/{APP}'
DATA_DIR = f'/var/lib/{APP}'
STAGING_DIR = '/media/developer/temp'
STAGING = f'{STAGING_DIR}/{APP}.ipk'
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


_HOST = re.compile(r'(?:[A-Za-z0-9_][A-Za-z0-9._-]*@)?[A-Za-z0-9][A-Za-z0-9._-]*')
_PRYSM_ADDRESS = re.compile(r'^https?://[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?/?$')
_LISTEN = re.compile(r'^[0-9.]+:[0-9]{1,5}$')
_MAC_ADDRESS = re.compile(r'[0-9A-Fa-f]{2}(:[0-9A-Fa-f]{2}){5}')


class Refused(ValueError):
    """A parameter a template does not accept."""


@dataclass(frozen=True)
class Datagram:
    """One UDP packet sent from `source` (this computer's address on the route to the receiver) to
    `address`:`port`, broadcast allowed."""

    source: str
    address: str
    port: int
    payload: bytes


@dataclass(frozen=True)
class Command:
    """One allow-listed command, ready to run.

    `until`/`fail` (regular expressions) end a held-stdin command early on a
    matching reply; `stdin_file` streams a local file to it. A `datagram`
    is sent instead of starting a process (`argv` then only names it)."""

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
    cwd: str | None = None
    datagram: Datagram | None = None

    def display(self) -> str:
        if self.datagram:
            sent = self.datagram
            return f'(UDP from {sent.source} to {sent.address}:{sent.port}) {" ".join(self.argv)}'
        text = shlex.join(self.argv)
        if self.stdin_file:
            text += ' < ' + shlex.quote(self.stdin_file)
        text = text.replace(str(ROOT) + '/', '').replace(str(ROOT), '.')
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


Member = TypeVar('Member', bound=StrEnum)


def _member(kind: type[Member], value: object, name: str) -> Member:
    """A member of `kind` itself, not its string."""
    if not isinstance(value, kind):
        raise Refused(f'{name}: {value!r}')
    return value


def _local_file(path: str | Path, name: str) -> str:
    path = Path(path)
    if '\0' in str(path) or str(path).startswith('-'):
        raise Refused(f'{name}: {path!s} is not accepted')
    return str(path)


# ---- the TV ----------------------------------------------------------------


def ssh_target(host: object) -> str:
    """`USER@HOST` (`root@TV`) or an ssh alias; never an option."""
    return _check(_HOST, host, 'ssh target')


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


def _luna(template: str, host: str, uri: str, payload: dict[str, object], app: bool = False) -> Command:
    """One Luna call to `uri` (a fixed service and method), as root or as Prysm's app."""
    # This TV's luna-send answers only while its stdin is open; the form used by install-ipk.sh.
    identity = f' -a {APP}' if app else ''
    remote = (
        f'timeout {LUNA_TIMEOUT} luna-send{identity} -n 1 '
        f'luna://{uri} {shlex.quote(json.dumps(payload, separators=(",", ":")))}'
    )
    return _ssh(template, host, remote, LUNA_TIMEOUT + 5, hold_stdin=True)


def _prysm(template: str, host: str, method: str, payload: dict[str, object], app: bool = False) -> Command:
    """A call to Prysm's own service."""
    return _luna(template, host, f'{SERVICE}/{method}', payload, app)


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
    """Processes whose executable lies under a path of Prysm's app or service, or of the earlier capturer."""
    remote = (
        'for e in /proc/[0-9]*/exe; do l=$(readlink "$e" 2>/dev/null) || continue; '
        f'case "$l" in */{APP}.*|*/{CAPTURE_APP}.*) p=${{e#/proc/}}; echo "${{p%/exe}} $l";; esac; done'
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


def _install_service(template: str, host: str, method: str, payload: dict[str, object], until: str) -> Command:
    """A call to the TV's install service for Prysm's app, held until it reports `until`."""
    request = shlex.quote(json.dumps({'id': APP, **payload, 'subscribe': True}, separators=(',', ':')))
    remote = f'timeout {INSTALL_TIMEOUT} luna-send -i luna://com.webos.appInstallService/dev/{method} {request}'
    return _ssh(
        template,
        host,
        remote,
        INSTALL_TIMEOUT + 10,
        hold_stdin=True,
        until=until,
        fail=r'"returnValue":\s*false|failed|FAILED',
    )


def tv_install(host: str) -> Command:
    """Install the staged package through the TV's install service."""
    return _install_service('tv.install', host, 'install', {'ipkUrl': STAGING}, r'"installed"')


def tv_unstage(host: str) -> Command:
    return _ssh('tv.unstage', host, f'rm -f {STAGING}', 15)


def tv_setup(host: str) -> Command:
    """The package's setup.sh: Luna registration, boot hook, rescan, activation."""
    return _ssh('tv.setup', host, SETUP, 90)


def tv_supervisor_uninstall(host: str) -> Command:
    """The supervisor's own uninstall: removes hook and manifests and rescans; it does not stop a running Prysm."""
    return _ssh('tv.supervisor-uninstall', host, f'{SUPERVISOR} uninstall', 90)


def tv_remove_app(host: str) -> Command:
    """Remove the app through the TV's install service (what the webOS CLI's `ares-install --remove` calls),
    after the supervisor's uninstall: the package has no removal script."""
    return _install_service('tv.remove-app', host, 'remove', {}, r'"removed"')


def tv_get_state(host: str) -> Command:
    return _prysm('tv.get-state', host, 'getState', {})


def tv_uninstall(host: str) -> Command:
    """The Luna `uninstall` (as the app): replies at once, stops the worker gracefully, then deregisters and
    exits, before every install and uninstall. The data directory stays."""
    return _prysm('tv.uninstall', host, 'uninstall', {}, app=True)


def tv_set_enabled(host: str, enabled: bool) -> Command:
    return _prysm('tv.set-enabled', host, 'setEnabled', {'enabled': _flag(enabled, 'enabled')}, app=True)


def tv_set_preferences(host: str, autostart: bool, prysm_address: str | None = None) -> Command:
    """Replace the preferences (the request is complete: autostart and, slim, prysm_address)."""
    payload: dict[str, object] = {'autostart': _flag(autostart, 'autostart')}
    if prysm_address is not None:
        payload['prysm_address'] = _check(_PRYSM_ADDRESS, prysm_address, 'prysm_address')
    return _prysm('tv.set-preferences', host, 'setPreferences', payload, app=True)


def tv_foreground(host: str) -> Command:
    """The app in front, as the application manager reports it (read-only)."""
    return _luna('tv.foreground', host, f'{APP_MANAGER}/getForegroundAppInfo', {})


def tv_launch_app(host: str) -> Command:
    """Bring Prysm's own app to the front (no other app: the id is fixed)."""
    return _luna('tv.launch-app', host, f'{APP_MANAGER}/launch', {'id': APP})


def tv_launch_home(host: str) -> Command:
    """Bring the TV's home screen to the front, over whatever app is there (the id is fixed)."""
    return _luna('tv.launch-home', host, f'{APP_MANAGER}/launch', {'id': HOME_APP})


def tv_close_app(host: str) -> Command:
    """Close Prysm's own app (no other app: the id is fixed)."""
    return _luna('tv.close-app', host, f'{APP_MANAGER}/closeByAppId', {'id': APP})


# ---- this computer ---------------------------------------------------------


def wake_on_lan(mac: str, source: str) -> Command:
    """The wake-on-LAN magic packet for the TV's hardware address `mac` (`[tv] mac`): six 0xFF bytes and the
    address 16 times, broadcast to UDP port 9 from `source`, this computer's address towards the TV."""
    checked = _check(_MAC_ADDRESS, mac, 'tv.mac')
    payload = b'\xff' * 6 + bytes.fromhex(checked.replace(':', '')) * 16
    datagram = Datagram(_ipv4(source), '255.255.255.255', 9, payload)
    return Command('local.wake-on-lan', ('wake-on-LAN magic packet for', checked), 5, datagram=datagram)


def verify_ipk(ipk: str) -> Command:
    """`ipk.py verify`: the package installs nothing outside the app and service directories."""
    return Command(
        'local.verify-ipk',
        ('python3', str(ROOT / 'packaging/webos/ipk.py'), 'verify', _local_file(ipk, 'package')),
        120,
    )


def git_last_commit() -> Command:
    """The checkout's last commit and when it was made (ISO 8601), to warn about a stale artifact."""
    return Command('local.git-last-commit', ('git', '-C', str(ROOT), 'log', '-1', '--format=%H %cI'), 10)


def headless(binary: str, settings: str, listen: str) -> Command:
    """prysm-headless on `listen` with --metrics, until stopped (SIGTERM)."""
    argv = (
        _local_file(binary, 'headless'),
        _local_file(settings, 'settings'),
        '--listen',
        _check(_LISTEN, listen, 'listen'),
        '--metrics',
    )
    return Command('local.headless', argv, 0, background=True, env={'RUST_LOG': 'warn,prysm=info'})


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


# ---- a Linux target over SSH ------------------------------------------------

# Where a run's files live on the target; a run directory is REMOTE_ROOT/RUN.
REMOTE_ROOT = '$HOME/.prysm-acceptance'
_RUN = re.compile(r'[A-Za-z0-9][A-Za-z0-9_-]{0,95}')
_V4L2 = re.compile(r'/dev/[A-Za-z0-9_.:+-]+(/[A-Za-z0-9_.:+-]+)*')
STOP_SECONDS = 30


class RunFile(StrEnum):
    """The files the runner puts into a run directory besides the artifact."""

    SETTINGS = 'settings.json'
    CLIP = 'clip'
    COMPOSE = 'compose.yaml'  # the image cell: packaging/docker/compose.yaml as it is
    OVERRIDE = 'compose.acceptance.yaml'  # the image cell: the runner's values


class Signal(StrEnum):
    TERM = 'TERM'
    KILL = 'KILL'


def run_dir(run: object) -> str:
    """The run directory on the target for the run named `run` (letters, digits, - and _)."""
    return f'{REMOTE_ROOT}/{_check(_RUN, run, "run")}'


def v4l2_device(path: object) -> str:
    """A V4L2 device path under /dev (a /dev/v4l/by-id link, or /dev/videoN); no `..`."""
    checked = _check(_V4L2, path, 'linux.device')
    if '..' in checked.split('/'):
        raise Refused(f'linux.device: {path!r} is not accepted')
    return checked


def _ipv4(address: object) -> str:
    if not isinstance(address, str):
        raise Refused(f'address: {address!r}')
    try:
        return str(ipaddress.IPv4Address(address))
    except ValueError:
        raise Refused(f'address: {address!r} is not an IPv4 address') from None


def linux_probe(host: str) -> Command:
    """The kernel name and the machine (`Linux aarch64`); also proves ssh works without a prompt."""
    return _ssh('linux.probe', host, 'uname -sm', 15)


def linux_ffmpeg_version(host: str) -> Command:
    """The first line of `ffmpeg -version` as Prysm will find it (the same non-interactive PATH), or `absent`."""
    return _ssh(
        'linux.ffmpeg-version', host, 'if command -v ffmpeg >/dev/null; then ffmpeg -version; else echo absent; fi', 15
    )


def linux_device(host: str, device: str) -> Command:
    """The device node a readable `device` resolves to (as Prysm lists it), or `unreadable`."""
    path = v4l2_device(device)
    return _ssh('linux.device', host, f'if [ -r {path} ]; then readlink -f {path}; else echo unreadable; fi', 15)


def linux_port_free(host: str, port: int) -> Command:
    """Sockets listening on TCP `port` (none: free)."""
    return _ssh('linux.port-free', host, f'ss -Htln sport = :{_number(port, "port", 1, 65535)}', 15)


def linux_deliver(host: str, run: str, binary: str) -> Command:
    """A new run directory with the binary in it, executable; prints the directory's absolute path and the
    target's clock (epoch seconds)."""
    directory = run_dir(run)
    remote = (
        f'mkdir -p {REMOTE_ROOT} && mkdir -m 700 {directory} && cat > {directory}/prysm-headless && '
        f'chmod 700 {directory}/prysm-headless && cd {directory} && pwd -P && date +%s'
    )
    return _ssh('linux.deliver', host, remote, 300, stdin_file=_local_file(binary, 'binary'))


def linux_put(host: str, run: str, file: RunFile, local: str) -> Command:
    """Copy a local file into the run directory as `file`."""
    name = _member(RunFile, file, 'file')
    return _ssh('linux.put', host, f'cat > {run_dir(run)}/{name}', 300, stdin_file=_local_file(local, 'file'))


def linux_start(host: str, run: str, listen: str) -> Command:
    """Start the binary detached, in its run directory, on `listen` with --metrics.

    A wrapper records its PID (headless.pid) and, when it exits, its exit
    status (exit-code); its output goes to headless.log. ssh returns at once."""
    directory = run_dir(run)
    wrapper = (
        f'cd {directory} || exit 1; '
        f'./prysm-headless {RunFile.SETTINGS} --listen {_check(_LISTEN, listen, "listen")} --metrics '
        '> headless.log 2>&1 < /dev/null & echo $! > headless.pid; wait $!; echo $? > exit-code'
    )
    remote = f"RUST_LOG=warn,prysm=info nohup sh -c '{wrapper}' > /dev/null 2>&1 < /dev/null &"
    return _ssh('linux.start', host, remote, 15)


def _alive(directory: str, pid: str) -> str:
    """A test that `pid` is the run's own binary (not a process that took its PID since)."""
    return f'[ "$(readlink /proc/{pid}/exe)" = "$(readlink -f {directory}/prysm-headless)" ]'


def linux_status(host: str, run: str) -> Command:
    """`exited CODE`, `running PID` or `starting`."""
    directory = run_dir(run)
    remote = (
        f'p=$(cat {directory}/headless.pid 2>/dev/null); '
        f'if [ -s {directory}/exit-code ]; then echo "exited $(cat {directory}/exit-code)"; '
        f'elif [ -n "$p" ] && {_alive(directory, "$p")}; then echo "running $p"; else echo starting; fi'
    )
    return _ssh('linux.status', host, remote, 15)


def linux_signal(host: str, run: str, pid: int, signal: Signal) -> Command:
    """Send `signal` to `pid` only while it is the run's own binary; prints `sent` or `gone`."""
    name = _member(Signal, signal, 'signal')
    number = str(_number(pid, 'pid', 2, 4194304))
    remote = f'if {_alive(run_dir(run), number)}; then kill -{name} {number} && echo sent; else echo gone; fi'
    return _ssh('linux.signal', host, remote, 15)


def linux_wait_exit(host: str, run: str, seconds: int = STOP_SECONDS) -> Command:
    """Wait up to `seconds` for the exit status; prints it, or `running`."""
    directory = run_dir(run)
    seconds = _number(seconds, 'seconds', 1, 120)
    remote = (
        f'i=0; while [ ! -s {directory}/exit-code ] && [ $i -lt {seconds} ]; do sleep 1; i=$((i+1)); done; '
        f'if [ -s {directory}/exit-code ]; then cat {directory}/exit-code; else echo running; fi'
    )
    return _ssh('linux.wait-exit', host, remote, seconds + 15)


def linux_read_log(host: str, run: str) -> Command:
    directory = run_dir(run)
    return _ssh(
        'linux.read-log', host, f'if [ -f {directory}/headless.log ]; then cat {directory}/headless.log; fi', 60
    )


def linux_kernel(host: str, since: int) -> Command:
    """The kernel log since `since` (the target's epoch seconds); the runner keeps the USB lines."""
    since = _number(since, 'since', 1, 2**40)
    return _ssh('linux.kernel', host, f'journalctl -k -q --no-pager -o short-iso --since @{since}', 60)


def linux_route(host: str, address: str) -> Command:
    """The target's route to `address` (IPv4): its `src` is the sender WLED sees."""
    return _ssh('linux.route', host, f'ip -4 route get {_ipv4(address)}', 15)


def linux_remove(host: str, run: str) -> Command:
    """Remove the run directory (and the runs' parent when it is left empty)."""
    remote = f'rm -rf {run_dir(run)}; rmdir {REMOTE_ROOT} 2>/dev/null; true'
    return _ssh('linux.remove', host, remote, 60)


# ---- Prysm's colours stream, read on the device Prysm runs on (a Linux target, the TV) -----

# Reads `GET URL` (a colours stream) for SECONDS from its start, each `data:` line printed as `<seconds since the
# start> <payload>` by the device's monotonic clock. Lines are read through http.client's de-chunking
# (`response.readline`), so an event split across chunks is one line and the body's last chunk ends the stream.
# It exits 0 at the end of the window, non-zero on another status than 200, a failed connection or a stream that
# ends sooner (the local reader, `web.events`, returns what it read then: the two differ on an early end on
# purpose). Python 3 without newer syntax, as the TV's may be old. As the TV's remote text it obeys the
# confinement rules (test_acceptance_commands.confined): no redirection (`>`) and no absolute path; and it holds
# no single quote, so `shlex.quote` wraps it in one pair as it is (a test asserts that).
COLOURS_READER = '\n'.join(
    (
        'import http.client, socket, sys, time',
        'from urllib.parse import urlsplit',
        'url = urlsplit(sys.argv[1])',
        'start = time.monotonic()',
        'end = start + float(sys.argv[2])',
        'connection = http.client.HTTPConnection(url.hostname, url.port, timeout=5)',
        'connection.request("GET", url.path, headers={"Accept": "text/event-stream"})',
        'sock = connection.sock',
        'response = connection.getresponse()',
        'if response.status != 200:',
        '    sys.exit("HTTP %d" % response.status)',
        'while time.monotonic() < end:',
        '    sock.settimeout(max(0.05, end - time.monotonic()))',
        '    try:',
        '        line = response.readline()',
        '    except socket.timeout:',
        '        break',
        '    if not line:',
        '        sys.exit("the stream ended %.3f s after the start" % (time.monotonic() - start))',
        '    if line.startswith(b"data:"):',
        '        print("%.3f %s" % (time.monotonic() - start, line[5:].strip().decode()), flush=True)',
    )
)
# How long the device's `timeout` lets the reader run past its window, and ssh past that.
READER_GRACE = 10
SSH_GRACE = 15


def device_reader_tools(host: str) -> Command:
    """Whether the device has what `device.colours` runs: one `missing NAME` line for each of python3 and
    `timeout` the shell does not find (nothing: both there)."""
    remote = 'for t in python3 timeout; do command -v "$t" || echo "missing $t"; done'
    return _ssh('device.reader-tools', host, remote, 15)


def _window(value: object) -> None:
    """A window of more than 0 and at most 120 s, whole or not; never a bool."""
    if not isinstance(value, (int, float)) or isinstance(value, bool) or not 0 < value <= 120:
        raise Refused(f'seconds: {value!r}')


def device_colours(host: str, port: int, seconds: float, address: str = '127.0.0.1') -> Command:
    """Prysm's colours stream at `address`:`port` on the device itself (loopback unless Prysm listens on one
    address only), read for `seconds` by `COLOURS_READER` and stamped by the device's clock; the device's
    `timeout` ends it `READER_GRACE` s after the window at the latest."""
    _window(seconds)
    url = f'http://{_ipv4(address)}:{_number(port, "port", 1, 65535)}/api/v1/colors/stream'
    bound = math.ceil(seconds) + READER_GRACE
    remote = f'timeout {bound} python3 -c {shlex.quote(COLOURS_READER)} {url} {seconds:g}'
    return _ssh('device.colours', host, remote, bound + SSH_GRACE)


def _artifact_dir(run: str, file: str) -> str:
    """A new run directory with `file` from stdin in it; prints the directory's absolute path and the target's clock."""
    directory = run_dir(run)
    return (
        f'mkdir -p {REMOTE_ROOT} && mkdir -m 700 {directory} && cat > {directory}/{file} && '
        f'cd {directory} && pwd -P && date +%s'
    )


# ---- the Debian package on a Linux target over SSH (the deb cell) -----------

DEB_PACKAGE = 'prysm-headless'
DEB_UNIT = 'prysm-headless.service'
DEB_SETTINGS = '/var/lib/prysm/settings.json'
DEB_FILE = 'prysm-headless.deb'
# The package's options file (a conffile): purge deletes it, so `installed = "replace"` backs it up and
# `settings restore` writes it back (`deb.options`, `deb.options-write`, `deb.options-remove`).
DEB_OPTIONS = '/etc/default/prysm-headless'
# A runtime drop-in that replaces the unit's command; /run is emptied at every boot. The options file stays as
# installed.
DEB_DROPIN_DIR = '/run/systemd/system/prysm-headless.service.d'
DEB_DROPIN = DEB_DROPIN_DIR + '/prysm-acceptance.conf'
# Prysm's files on a Linux target, none of which may be there before the run or after the purge: what the
# package installs (packaging/debian/build-deb.sh, postinst), the unit's state (a
# DynamicUser's, under /var/lib/private), a unit of the admin's, and the run's drop-in.
DEB_PATHS = (
    '/usr/bin/prysm-headless',
    '/usr/lib/systemd/system/prysm-headless.service',
    '/usr/share/doc/prysm-headless',
    DEB_OPTIONS,
    '/etc/systemd/system/prysm-headless.service',
    '/etc/systemd/system/prysm-headless.service.d',
    '/etc/systemd/system/multi-user.target.wants/prysm-headless.service',
    '/var/lib/prysm',
    '/var/lib/private/prysm',
    DEB_DROPIN_DIR,
)
# The user and group systemd gives the unit while it runs (DynamicUser); none may be there before or after the run.
DEB_ACCOUNT = 'prysm-headless'
_APT = 'sudo -n env DEBIAN_FRONTEND=noninteractive apt-get'


class UnitAction(StrEnum):
    STOP = 'stop'
    RESTART = 'restart'


def deb_sudo(host: str) -> Command:
    """Passwordless sudo works (exit 0)."""
    return _ssh('deb.sudo', host, 'sudo -n true', 15)


def deb_installed(host: str) -> Command:
    """dpkg's status and version of the package: `installed 1.0.0~alpha.0-1`, or `not-installed`."""
    remote = f"dpkg-query -W -f='${{db:Status-Status}} ${{Version}}\\n' {DEB_PACKAGE} 2>/dev/null || echo not-installed"
    return _ssh('deb.installed', host, remote, 15)


def deb_unit(host: str) -> Command:
    """The unit's state and its main process's exit, as KEY=VALUE lines."""
    names = ('LoadState', 'ActiveState', 'SubState', 'Result', 'ExecMainCode', 'ExecMainStatus', 'MainPID')
    return _ssh('deb.unit', host, f'systemctl show {" ".join(f"-p {name}" for name in names)} {DEB_UNIT}', 15)


def deb_deliver(host: str, run: str, deb: str) -> Command:
    """A new run directory with the package in it (`DEB_FILE`); prints its absolute path and the target's clock."""
    return _ssh('deb.deliver', host, _artifact_dir(run, DEB_FILE), 300, stdin_file=_local_file(deb, 'package'))


def deb_install(host: str, run: str) -> Command:
    """Install the package from the run directory with apt-get, non-interactively and without recommends."""
    remote = f'cd {run_dir(run)} && {_APT} install -y --no-install-recommends ./{DEB_FILE}'
    return _ssh('deb.install', host, remote, 600)


def deb_dropin(host: str, listen: str) -> Command:
    """The runtime drop-in: the unit's own command with `--listen LISTEN --metrics`; then daemon-reload."""
    start = f'ExecStart=/usr/bin/prysm-headless {DEB_SETTINGS} --listen {_check(_LISTEN, listen, "listen")} --metrics'
    remote = (
        f"sudo -n mkdir -p {DEB_DROPIN_DIR} && printf '%s\\n' '[Service]' 'ExecStart=' '{start}' "
        f'| sudo -n tee {DEB_DROPIN} > /dev/null && sudo -n systemctl daemon-reload'
    )
    return _ssh('deb.dropin', host, remote, 30)


def deb_systemctl(host: str, action: UnitAction) -> Command:
    """Stop or restart the unit; systemctl waits (a stop sends SIGTERM, KILL after TimeoutStopSec 30 s)."""
    return _ssh('deb.systemctl', host, f'sudo -n systemctl {_member(UnitAction, action, "action")} {DEB_UNIT}', 60)


def deb_main_user(host: str) -> Command:
    """The user the unit's main process runs as."""
    return _ssh('deb.main-user', host, f'ps -o user= -p "$(systemctl show -p MainPID --value {DEB_UNIT})"', 15)


def deb_journal(host: str, since: int) -> Command:
    """The unit's journal since `since` (the target's epoch seconds)."""
    since = _number(since, 'since', 1, 2**40)
    return _ssh(
        'deb.journal', host, f'sudo -n journalctl -u {DEB_UNIT} -q --no-pager -o short-iso --since @{since}', 60
    )


def deb_purge(host: str) -> Command:
    """Remove the runtime drop-in and purge the package (a no-op when it is not installed); then daemon-reload."""
    remote = (
        f'sudo -n rm -f {DEB_DROPIN}; sudo -n rmdir {DEB_DROPIN_DIR} 2>/dev/null; '
        f'{_APT} purge -y {DEB_PACKAGE}; s=$?; sudo -n systemctl daemon-reload; exit $s'
    )
    return _ssh('deb.purge', host, remote, 600)


def deb_options(host: str) -> Command:
    """The options file as it is, or `__absent__`."""
    remote = f'if [ -f {DEB_OPTIONS} ]; then cat {DEB_OPTIONS}; else echo __absent__; fi'
    return _ssh('deb.options', host, remote, 15)


def deb_options_write(host: str, local: str | Path) -> Command:
    """The options file replaced by the local file `local` (root's, 0644, as the package installs it)."""
    remote = f'sudo -n tee {DEB_OPTIONS} > /dev/null && sudo -n chmod 0644 {DEB_OPTIONS}'
    return _ssh('deb.options-write', host, remote, 30, stdin_file=_local_file(local, 'options'))


def deb_options_remove(host: str) -> Command:
    """The options file removed, as an administrator who deleted it left it."""
    return _ssh('deb.options-remove', host, f'sudo -n rm -f {DEB_OPTIONS}', 30)


def deb_leftovers(host: str) -> Command:
    """What of Prysm is on the target, one per line: each of `DEB_PATHS` that exists, `user prysm-headless` and
    `group prysm-headless` when they exist, and dpkg's status of the package when dpkg knows it. The paths are tested
    with sudo: /var/lib/private is root's alone."""
    remote = (
        f'for p in {" ".join(DEB_PATHS)}; do '
        'if sudo -n test -e "$p" || sudo -n test -L "$p"; then echo "$p"; fi; done; '
        f"getent passwd {DEB_ACCOUNT} > /dev/null && echo 'user {DEB_ACCOUNT}'; "
        f"getent group {DEB_ACCOUNT} > /dev/null && echo 'group {DEB_ACCOUNT}'; "
        f"dpkg-query -W -f='${{db:Status-Status}}\\n' {DEB_PACKAGE} 2>/dev/null; true"
    )
    return _ssh('deb.leftovers', host, remote, 15)


# ---- the Docker image on a Linux target over SSH (the image cell) -----------

# The name `cargo xtask dist image-*` gives the image, which packaging/docker/compose.yaml runs.
IMAGE_TAG = 'prysm-headless:local'
IMAGE_FILE = 'prysm-headless.tar'
# The port compose.yaml publishes (the portal, on Prysm's default listen address).
IMAGE_PORT = 8787
_IMAGE_INSPECT = f"docker image inspect -f '{{{{.Id}}}}' {IMAGE_TAG}"


class ComposeAction(StrEnum):
    """What `image.compose` runs in the run's project (the service `prysm`)."""

    SEED = 'seed'  # the settings into the volume, as the image's user (as smoke.sh does)
    UP = 'up'
    STATE = 'state'
    STOP = 'stop'
    RECREATE = 'recreate'
    LOGS = 'logs'
    DOWN = 'down'  # with the project's volumes and network


_COMPOSE = {
    ComposeAction.SEED: (
        f"run --rm -T --no-deps --entrypoint sh prysm -c 'cat > /data/settings.json' < {RunFile.SETTINGS}",
        120,
    ),
    ComposeAction.UP: ('up -d --pull never --no-build', 120),
    ComposeAction.STATE: ("ps -a --format '{{.State}} {{.ExitCode}}' prysm", 30),
    ComposeAction.STOP: ('stop prysm', 60),
    ComposeAction.RECREATE: ('up -d --pull never --no-build --force-recreate', 120),
    ComposeAction.LOGS: ('logs --no-color --timestamps prysm', 60),
    ComposeAction.DOWN: ('down --volumes --remove-orphans', 120),
}


def compose_project(run: object) -> str:
    """The compose project of the run named `run`: `prysm-acceptance-RUN` in lower case."""
    return f'prysm-acceptance-{_check(_RUN, run, "run").lower()}'


def image_docker(host: str) -> Command:
    """The Docker server's version, then the compose plugin's; fails when either does not answer."""
    return _ssh('image.docker', host, "docker info --format '{{.ServerVersion}}' && docker compose version --short", 30)


def image_device(host: str, device: str) -> Command:
    """The group ID of the node `device` resolves to, or `absent`."""
    path = v4l2_device(device)
    return _ssh('image.device', host, f'if [ -e {path} ]; then stat -L -c %g {path}; else echo absent; fi', 15)


def image_present(host: str, run: str) -> Command:
    """What exists of the image tag and of the run's compose project (containers, volumes); nothing: all free."""
    label = f'label=com.docker.compose.project={compose_project(run)}'
    remote = f'{_IMAGE_INSPECT} 2>/dev/null; docker ps -aq --filter {label}; docker volume ls -q --filter {label}; true'
    return _ssh('image.present', host, remote, 30)


def image_deliver(host: str, run: str, image: str) -> Command:
    """A new run directory with the image archive in it (`IMAGE_FILE`); prints its path and the target's clock."""
    return _ssh('image.deliver', host, _artifact_dir(run, IMAGE_FILE), 600, stdin_file=_local_file(image, 'image'))


def image_load(host: str, run: str) -> Command:
    """`docker load` the archive; prints the ID `IMAGE_TAG` names then."""
    return _ssh('image.load', host, f'docker load -i {run_dir(run)}/{IMAGE_FILE} && {_IMAGE_INSPECT}', 600)


def image_compose(host: str, run: str, action: ComposeAction) -> Command:
    """`docker compose` in the run directory with compose.yaml and the runner's override, project `compose_project`."""
    words, timeout = _COMPOSE[_member(ComposeAction, action, 'action')]
    remote = (
        f'cd {run_dir(run)} && docker compose -p {compose_project(run)} '
        f'-f {RunFile.COMPOSE} -f {RunFile.OVERRIDE} {words}'
    )
    return _ssh('image.compose', host, remote, timeout)


def image_remove(host: str) -> Command:
    """Remove `IMAGE_TAG` (the preflight found none, so it is the run's), also after an interrupted load; prints
    `gone` when there is none."""
    remote = f'if {_IMAGE_INSPECT} > /dev/null 2>&1; then docker image rm {IMAGE_TAG}; else echo gone; fi'
    return _ssh('image.remove', host, remote, 120)


# ---- the build system (--prepare) -------------------------------------------

# The `cargo xtask dist` targets (crates/tools/xtask/src/dist.rs) the runner may ask for.
XTASK_TARGETS = (
    'mac',
    'linux-arm64',
    'linux-amd64',
    'deb-arm64',
    'deb-amd64',
    'image-arm64',
    'image-amd64',
    'tv-full',
    'tv-slim',
)


def xtask_dist(targets: Sequence[str]) -> Command:
    """`cargo xtask dist TARGET...` in the repository: the artifacts under their names in dist/."""
    if not targets or any(target not in XTASK_TARGETS for target in targets):
        raise Refused(f'xtask dist targets: {list(targets)!r}')
    return Command('local.xtask-dist', ('cargo', 'xtask', 'dist', *targets), 3600, cwd=str(ROOT))


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
    'tv.remove-app': lambda: tv_remove_app('root@tv.example'),
    'tv.get-state': lambda: tv_get_state('root@tv.example'),
    'tv.uninstall': lambda: tv_uninstall('root@tv.example'),
    'tv.set-enabled': lambda: tv_set_enabled('root@tv.example', True),
    'tv.set-preferences': lambda: tv_set_preferences('root@tv.example', False, 'http://192.0.2.20:8787'),
    'tv.foreground': lambda: tv_foreground('root@tv.example'),
    'tv.launch-app': lambda: tv_launch_app('root@tv.example'),
    'tv.launch-home': lambda: tv_launch_home('root@tv.example'),
    'tv.close-app': lambda: tv_close_app('root@tv.example'),
    'linux.probe': lambda: linux_probe('pi@pi.example'),
    'linux.ffmpeg-version': lambda: linux_ffmpeg_version('pi@pi.example'),
    'linux.device': lambda: linux_device('pi@pi.example', '/dev/v4l/by-id/usb-EXAMPLE-video-index0'),
    'linux.port-free': lambda: linux_port_free('pi@pi.example', 18787),
    'linux.deliver': lambda: linux_deliver('pi@pi.example', 'RUN-CELL', 'dist/prysm-headless-linux-arm64'),
    'linux.put': lambda: linux_put('pi@pi.example', 'RUN-CELL', RunFile.SETTINGS, 'headless-settings.json'),
    'linux.start': lambda: linux_start('pi@pi.example', 'RUN-CELL', '0.0.0.0:18787'),
    'linux.status': lambda: linux_status('pi@pi.example', 'RUN-CELL'),
    'linux.signal': lambda: linux_signal('pi@pi.example', 'RUN-CELL', 4242, Signal.TERM),
    'linux.wait-exit': lambda: linux_wait_exit('pi@pi.example', 'RUN-CELL'),
    'linux.read-log': lambda: linux_read_log('pi@pi.example', 'RUN-CELL'),
    'linux.kernel': lambda: linux_kernel('pi@pi.example', 1763380000),
    'linux.route': lambda: linux_route('pi@pi.example', '192.0.2.40'),
    'linux.remove': lambda: linux_remove('pi@pi.example', 'RUN-CELL'),
    'device.reader-tools': lambda: device_reader_tools('pi@pi.example'),
    'device.colours': lambda: device_colours('pi@pi.example', 18787, 10.0),
    'deb.sudo': lambda: deb_sudo('pi@pi.example'),
    'deb.installed': lambda: deb_installed('pi@pi.example'),
    'deb.unit': lambda: deb_unit('pi@pi.example'),
    'deb.deliver': lambda: deb_deliver('pi@pi.example', 'RUN-CELL', 'dist/prysm-headless-linux-arm64.deb'),
    'deb.install': lambda: deb_install('pi@pi.example', 'RUN-CELL'),
    'deb.dropin': lambda: deb_dropin('pi@pi.example', '0.0.0.0:18787'),
    'deb.systemctl': lambda: deb_systemctl('pi@pi.example', UnitAction.RESTART),
    'deb.main-user': lambda: deb_main_user('pi@pi.example'),
    'deb.journal': lambda: deb_journal('pi@pi.example', 1763380000),
    'deb.purge': lambda: deb_purge('pi@pi.example'),
    'deb.leftovers': lambda: deb_leftovers('pi@pi.example'),
    'deb.options': lambda: deb_options('pi@pi.example'),
    'deb.options-write': lambda: deb_options_write('pi@pi.example', 'work/prysm-headless.options'),
    'deb.options-remove': lambda: deb_options_remove('pi@pi.example'),
    'image.docker': lambda: image_docker('pi@pi.example'),
    'image.device': lambda: image_device('pi@pi.example', '/dev/v4l/by-id/usb-EXAMPLE-video-index0'),
    'image.present': lambda: image_present('pi@pi.example', 'RUN-CELL'),
    'image.deliver': lambda: image_deliver('pi@pi.example', 'RUN-CELL', 'dist/prysm-headless-linux-arm64.tar'),
    'image.load': lambda: image_load('pi@pi.example', 'RUN-CELL'),
    'image.compose': lambda: image_compose('pi@pi.example', 'RUN-CELL', ComposeAction.UP),
    'image.remove': lambda: image_remove('pi@pi.example'),
    'local.wake-on-lan': lambda: wake_on_lan('00:00:5E:00:53:01', '192.0.2.10'),
    'local.verify-ipk': lambda: verify_ipk('dist/prysm-tv-full.ipk'),
    'local.git-last-commit': git_last_commit,
    'local.xtask-dist': lambda: xtask_dist(['linux-arm64', 'deb-arm64']),
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
