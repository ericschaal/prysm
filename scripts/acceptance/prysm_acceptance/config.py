"""The runner's configuration: a TOML file (see config.example.toml), checked into typed values."""

from collections.abc import Mapping
from dataclasses import dataclass, field, fields
from enum import StrEnum
from pathlib import Path
import re
import tomllib
from typing import Any, TypeAlias, TypeVar, cast

from prysm_acceptance import commands
from prysm_acceptance.options import Delivery, Flavour, Platform

_ADDRESS = re.compile(r'[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?')
_URL = re.compile(r'https?://[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?/?')
LOCAL = 'local'
SSH = 'ssh://'
# Where Prysm listens on a target over SSH unless `listen` says otherwise: the LAN, so this computer reaches it.
REMOTE_LISTEN = '0.0.0.0:18787'

# The TOML document as tomllib reads it. Every value is checked here before it
# becomes a typed field.
Toml: TypeAlias = Mapping[str, Any]


class ConfigError(ValueError):
    pass


class Installed(StrEnum):
    """What a cell does with a Prysm it finds on its target (`[tv] installed`, `[linux] installed`): there is
    none before a run and none after it."""

    REFUSE = 'refuse'  # stop before changing anything, saying what was found
    REPLACE = 'replace'  # back up its settings, verify the backup, uninstall it; never reinstalled


@dataclass
class Leds:
    top: int = 0
    right: int = 0
    bottom: int = 0
    left: int = 0

    def as_settings(self) -> dict[str, int]:
        return {'top': self.top, 'right': self.right, 'bottom': self.bottom, 'left': self.left}


@dataclass
class Checks:
    """`[checks]`: how long the frame-rate checks sample and how far the rate may stray."""

    seconds: int = 10
    fps_tolerance: float = 0.15


@dataclass
class Capture:
    """`[PLATFORM.capture]`: the device source's size and rate, always sent (640x360 at 30 fps unless set)."""

    width: int = 640
    height: int = 360
    fps: int = 30


@dataclass
class CastFeed:
    """`[tv.feed.cast]`: the clip served from this computer and cast to the TV's own Cast receiver."""

    serve_port: int = 18792
    settle_seconds: float = 5.0


@dataclass
class ChromecastFeed(CastFeed):
    """`[PLATFORM.feed.chromecast]`: the Chromecast whose HDMI output the capture card records, reached over ADB."""

    adb_serial: str = ''


@dataclass
class Machine:
    """`[mac]` or `[linux]`: where and from which artifacts a host cell runs Prysm."""

    target: str = LOCAL
    binary: str = ''
    deb: str = ''
    image: str = ''
    device: str = ''  # what Prysm's device setting takes: an AVFoundation unique ID, or a V4L2 path
    listen: str = '127.0.0.1:18787'
    display_port: int = 18790
    capture: Capture = field(default_factory=Capture)
    feed: ChromecastFeed | None = None  # None: no controlled feed into the capture card
    installed: Installed = Installed.REFUSE  # Linux only: the package cell's rule for an installed Prysm

    def artifact(self, delivery: Delivery = Delivery.BINARY) -> Path | None:
        """The configured artifact of `delivery` (`binary`, `deb` or `image`), relative to the repository; None
        without one."""
        name = {Delivery.BINARY: self.binary, Delivery.DEB: self.deb, Delivery.IMAGE: self.image}[delivery]
        return commands.ROOT / name if name else None


@dataclass
class Tv:
    """`[tv]`: the TV over SSH and the packages under test."""

    target: str = ''  # ssh://root@HOST
    # The packages `cargo xtask dist tv-full tv-slim` writes (docs/developers/packaging.md).
    ipk_full: str = 'dist/prysm-tv-full.ipk'
    ipk_slim: str = 'dist/prysm-tv-slim.ipk'
    mac: str = ''  # the TV's hardware address for wake-on-LAN; empty: an unreachable TV is not woken
    installed: Installed = Installed.REFUSE
    soak_seconds: int = 90
    feed: CastFeed = field(default_factory=CastFeed)  # the clip cast to the TV: always, the table only tunes it

    @property
    def ssh(self) -> str:
        """The target as ssh names it (root@HOST); empty when not configured."""
        return self.target.removeprefix(SSH)

    @property
    def host(self) -> str:
        """The TV's address: the target's HOST; empty when not configured."""
        return self.ssh.rsplit('@', 1)[-1]

    def ipk(self, flavour: Flavour) -> Path | None:
        """The configured package of `flavour`, relative to the repository; None without one."""
        name = self.ipk_full if flavour == Flavour.FULL else self.ipk_slim
        return commands.ROOT / name if name else None


@dataclass
class Config:
    wled: str = ''
    leds: Leds = field(default_factory=Leds)
    checks: Checks = field(default_factory=Checks)
    mac: Machine = field(default_factory=Machine)
    linux: Machine = field(default_factory=Machine)
    tv: Tv = field(default_factory=Tv)
    eyes_portal: str = ''

    def machine(self, platform: Platform) -> Machine:
        return self.mac if platform == Platform.MAC else self.linux


Section = TypeVar('Section', Leds, Checks, Capture, CastFeed, ChromecastFeed, Machine, Tv)
FeedSettings = TypeVar('FeedSettings', CastFeed, ChromecastFeed)


def _table(data: Toml, name: str, label: str) -> dict[str, Any]:
    table = data.get(name, {})
    if not isinstance(table, dict):
        raise ConfigError(f'[{label}] must be a table')
    return dict(cast('dict[str, Any]', table))


def _section(values: Mapping[str, Any], kind: type[Section], label: str) -> Section:
    """`values` as a `kind`: no unknown keys, each value of its default's type (a whole number may be a float)."""
    known = {item.name for item in fields(kind)}
    unknown = set(values) - known
    if unknown:
        raise ConfigError(f'[{label}]: unknown keys {sorted(unknown)}')
    defaults = kind()
    for key, value in values.items():
        expected = cast('type[object]', type(getattr(defaults, key)))
        if issubclass(expected, StrEnum):
            expected = str  # checked against its members in `parse`
        whole = isinstance(value, int) and not isinstance(value, bool)
        if expected is float and whole:
            continue
        if not isinstance(value, expected) or isinstance(value, bool) != (expected is bool):
            raise ConfigError(f'[{label}] {key}: expected {expected.__name__}')
    return kind(**values)


def _feed(table: dict[str, Any], label: str, kind: str, settings: type[FeedSettings]) -> FeedSettings | None:
    """`[LABEL.feed.KIND]` (the one feed `label` supports) as `settings`, or None without a feed. Takes the
    `feed` table out of `table`."""
    feeds = _table(table, 'feed', f'{label}.feed')
    table.pop('feed', None)
    unknown = set(feeds) - {kind}
    if unknown:
        raise ConfigError(f'[{label}.feed]: unknown feeds {sorted(unknown)} (supported: {kind})')
    if kind not in feeds:
        return None
    name = f'{label}.feed.{kind}'
    found = _section(_table(feeds, kind, name), settings, name)
    if not 1 <= found.serve_port <= 65535:
        raise ConfigError(f'{name}.serve_port: 1 to 65535')
    if not 0 <= found.settle_seconds <= 60:
        raise ConfigError(f'{name}.settle_seconds: 0 to 60')
    return found


def _chromecast(table: dict[str, Any], label: str) -> ChromecastFeed | None:
    """`[LABEL.feed.chromecast]`, or None without a feed."""
    chromecast = _feed(table, label, 'chromecast', ChromecastFeed)
    if chromecast is None:
        return None
    if not chromecast.adb_serial:
        raise ConfigError(f'{label}.feed.chromecast.adb_serial is required (the serial adb devices lists)')
    try:
        commands.adb_serial(chromecast.adb_serial)
    except commands.Refused as error:
        raise ConfigError(str(error)) from error
    return chromecast


def _machine(data: Toml, platform: Platform) -> Machine:
    """`[mac]` or `[linux]` with its `capture` and `feed` tables."""
    label = str(platform)
    table = _table(data, label, label)
    capture = _section(_table(table, 'capture', f'{label}.capture'), Capture, f'{label}.capture')
    feed = _chromecast(table, label)
    table.pop('capture', None)
    host = _section(table, Machine, label)
    host.capture = capture
    host.feed = feed
    _target(host, platform, listen_given='listen' in table)
    if platform == Platform.MAC and (host.deb or host.image):
        raise ConfigError('[mac]: deb and image are Linux deliveries')
    if platform == Platform.MAC and 'installed' in table:
        raise ConfigError('[mac] installed: only the Linux package cell installs Prysm')
    host.installed = _installed(host.installed, label)
    if platform == Platform.LINUX and host.device:
        try:
            commands.v4l2_device(host.device)
        except commands.Refused as error:
            raise ConfigError(f'[linux] device: a V4L2 path such as /dev/v4l/by-id/...; {error}') from error
    if not all(0 < value <= 4000 for value in vars(host.capture).values()):
        raise ConfigError(f'[{label}.capture]: width, height and fps 1 to 4000')
    return host


def _target(host: Machine, platform: Platform, listen_given: bool) -> None:
    """`target` ("local" or "ssh://USER@HOST", Linux only) and `listen` (on the LAN for a target over SSH)."""
    label = str(platform)
    if host.target.startswith(SSH):
        if platform == Platform.MAC:
            raise ConfigError(f'[mac] target {host.target!r}: a remote Mac is not implemented; use "local"')
        try:
            commands.ssh_target(host.target.removeprefix(SSH))
        except commands.Refused as error:
            raise ConfigError(f'[{label}] target: {error}') from error
        if not listen_given:
            host.listen = REMOTE_LISTEN
        if host.listen.startswith('127.'):
            raise ConfigError(f'[{label}] listen: {host.listen} on the target is out of reach from here')
    elif host.target != LOCAL:
        raise ConfigError(f'[{label}] target: "local" or "ssh://USER@HOST"')
    if not re.fullmatch(r'[0-9.]+:[0-9]{1,5}', host.listen):
        raise ConfigError(f'[{label}] listen: {host.listen!r} is not ADDRESS:PORT')


def address(value: str, name: str, required: bool = False) -> str:
    """`value` when it is host[:port] (or empty and not `required`)."""
    if not value:
        if required:
            raise ConfigError(f'{name} is required')
        return value
    if not _ADDRESS.fullmatch(value):
        raise ConfigError(f'{name}: {value!r} is not host[:port]')
    return value


def parse(data: Toml) -> Config:
    unknown = set(data) - {'wled', 'leds', 'checks', 'mac', 'linux', 'tv', 'eyes'}
    if unknown:
        raise ConfigError(f'unknown sections {sorted(unknown)}')
    config = Config(
        wled=address(_table(data, 'wled', 'wled').get('address', ''), 'wled.address'),
        leds=_section(_table(data, 'leds', 'leds'), Leds, 'leds'),
        checks=_section(_table(data, 'checks', 'checks'), Checks, 'checks'),
        mac=_machine(data, Platform.MAC),
        linux=_machine(data, Platform.LINUX),
        tv=_tv(_table(data, 'tv', 'tv')),
        eyes_portal=_table(data, 'eyes', 'eyes').get('portal', ''),
    )
    for name, count in config.leds.as_settings().items():
        if not 0 <= count <= 16384:
            raise ConfigError(f'leds.{name}: 0 to 16384')
    if not 0 < config.checks.fps_tolerance < 1:
        raise ConfigError('checks.fps_tolerance: between 0 and 1')
    _check_tv(config.tv)
    if config.eyes_portal and not _URL.fullmatch(config.eyes_portal):
        raise ConfigError('eyes.portal: http://host[:port]')
    return config


def _tv(table: dict[str, Any]) -> Tv:
    """`[tv]` with its `feed.cast` table (its defaults without one)."""
    feed = _feed(table, 'tv', 'cast', CastFeed) or CastFeed()
    tv = _section(table, Tv, 'tv')
    tv.feed = feed
    return tv


def _check_tv(tv: Tv) -> None:
    """`[tv]`: the ssh target, the hardware address for wake-on-LAN, the rule for an installed Prysm."""
    try:
        if tv.target:
            if not tv.target.startswith(SSH):
                raise ConfigError('[tv] target: "ssh://root@HOST"')
            commands.ssh_target(tv.ssh)
        if tv.mac:
            commands.wake_on_lan(tv.mac, '192.0.2.1')
    except commands.Refused as error:
        raise ConfigError(f'[tv] {error}') from error
    tv.installed = _installed(tv.installed, 'tv')


def _installed(value: str, label: str) -> Installed:
    """`[label] installed` as a member of `Installed`."""
    try:
        return Installed(value)
    except ValueError:
        raise ConfigError(f'[{label}] installed: "refuse" or "replace"') from None


def load(path: Path) -> Config:
    with path.open('rb') as file:
        try:
            return parse(tomllib.load(file))
        except tomllib.TOMLDecodeError as error:
            raise ConfigError(f'{path}: {error}') from error
