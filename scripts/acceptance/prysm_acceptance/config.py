"""The runner's configuration: a TOML file (see config.example.toml), checked into typed values."""

from collections.abc import Mapping
from dataclasses import dataclass, field, fields
from enum import StrEnum
from pathlib import Path
import re
import tomllib
from typing import Any, TypeAlias, TypeVar, cast

from prysm_acceptance import commands
from prysm_acceptance.commands import Flavour
from prysm_acceptance.options import Platform

_ADDRESS = re.compile(r'[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?')
_SIZE = re.compile(r'([0-9]{1,4})x([0-9]{1,4})')
_URL = re.compile(r'https?://[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?/?')
LOCAL = 'local'
SSH = 'ssh://'

# The TOML document as tomllib reads it. Every value is checked here before it
# becomes a typed field.
Toml: TypeAlias = Mapping[str, Any]


class ConfigError(ValueError):
    pass


class RestoreFlavour(StrEnum):
    """The package the `tv` stage leaves installed at the end."""

    AS_FOUND = 'as-found'
    FULL = 'full'
    SLIM = 'slim'
    NONE = 'none'


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
    """`[PLATFORM.capture]`: the capture size and rate of the device source (absent: Prysm's defaults)."""

    width: int = 640
    height: int = 360
    fps: int = 30


@dataclass
class ChromecastFeed:
    """`[PLATFORM.feed.chromecast]`: the Chromecast whose HDMI output the capture card records."""

    adb_serial: str = ''
    serve_port: int = 18792
    settle_seconds: float = 5.0


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
    capture: Capture | None = None
    feed: ChromecastFeed | None = None  # None: no controlled feed into the capture card

    def artifact(self) -> Path | None:
        """The configured binary, relative to the repository; None without one."""
        return commands.ROOT / self.binary if self.binary else None


@dataclass
class Tv:
    address: str = ''
    ssh: str = ''
    ares_device: str = ''
    flavours: list[Flavour] = field(default_factory=lambda: [Flavour.FULL, Flavour.SLIM])
    restore_flavour: RestoreFlavour = RestoreFlavour.AS_FOUND
    soak_seconds: int = 90
    # The packages `cargo xtask dist tv-full tv-slim` writes (docs/developers/packaging.md).
    ipk_full: str = 'dist/prysm-tv-full.ipk'
    ipk_slim: str = 'dist/prysm-tv-slim.ipk'

    def ipk(self, flavour: Flavour) -> str:
        """The configured package of `flavour`, relative to the repository."""
        return str(commands.ROOT / (self.ipk_full if flavour == Flavour.FULL else self.ipk_slim))


@dataclass
class Receiver:
    url: str = ''
    listen: str = '0.0.0.0:8788'
    advertise: str = ''
    wss_url: str = ''


@dataclass
class Sweep:
    sizes: list[str] = field(default_factory=lambda: ['320x180', '640x360', '960x540', '1280x720'])
    rates: list[int] = field(default_factory=lambda: [30, 60])
    seconds: int = 10

    def pairs(self) -> list[tuple[int, int, int]]:
        found: list[tuple[int, int, int]] = []
        for size in self.sizes:
            match = _SIZE.fullmatch(size)
            if match is None:
                raise ConfigError(f'sweep.sizes: {size!r} is not WIDTHxHEIGHT')
            width, height = int(match[1]), int(match[2])
            found += [(width, height, rate) for rate in self.rates]
        return found


@dataclass
class Config:
    wled: str = ''
    leds: Leds = field(default_factory=Leds)
    checks: Checks = field(default_factory=Checks)
    mac: Machine = field(default_factory=Machine)
    linux: Machine = field(default_factory=Machine)
    tv: Tv = field(default_factory=Tv)
    receiver: Receiver = field(default_factory=Receiver)
    sweep: Sweep = field(default_factory=Sweep)
    eyes_portal: str = ''

    def machine(self, platform: Platform) -> Machine:
        return self.mac if platform == Platform.MAC else self.linux


Section = TypeVar('Section', Leds, Checks, Capture, ChromecastFeed, Machine, Tv, Receiver, Sweep)


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


def _feed(table: dict[str, Any], label: str) -> ChromecastFeed | None:
    """`[LABEL.feed.chromecast]`, or None without a feed."""
    feeds = _table(table, 'feed', f'{label}.feed')
    unknown = set(feeds) - {'chromecast'}
    if unknown:
        raise ConfigError(f'[{label}.feed]: unknown feeds {sorted(unknown)} (supported: chromecast)')
    if 'chromecast' not in feeds:
        return None
    name = f'{label}.feed.chromecast'
    chromecast = _section(_table(feeds, 'chromecast', name), ChromecastFeed, name)
    if not chromecast.adb_serial:
        raise ConfigError(f'{name}.adb_serial is required (the serial adb devices lists)')
    try:
        commands.adb_serial(chromecast.adb_serial)
    except commands.Refused as error:
        raise ConfigError(str(error)) from error
    if not 1 <= chromecast.serve_port <= 65535:
        raise ConfigError(f'{name}.serve_port: 1 to 65535')
    if not 0 <= chromecast.settle_seconds <= 60:
        raise ConfigError(f'{name}.settle_seconds: 0 to 60')
    return chromecast


def _machine(data: Toml, platform: Platform) -> Machine:
    """`[mac]` or `[linux]` with its `capture` and `feed` tables."""
    label = str(platform)
    table = _table(data, label, label)
    capture = _section(_table(table, 'capture', f'{label}.capture'), Capture, f'{label}.capture')
    has_capture = 'capture' in table
    feed = _feed(table, label)
    table.pop('capture', None)
    table.pop('feed', None)
    host = _section(table, Machine, label)
    host.capture = capture if has_capture else None
    host.feed = feed
    if host.target.startswith(SSH):
        if platform == Platform.MAC:
            raise ConfigError(f'[mac] target {host.target!r}: a remote Mac is not implemented; use "local"')
        try:
            commands.ssh_target(host.target.removeprefix(SSH))
        except commands.Refused as error:
            raise ConfigError(f'[{label}] target: {error}') from error
    elif host.target != LOCAL:
        raise ConfigError(f'[{label}] target: "local" or "ssh://USER@HOST"')
    if platform == Platform.MAC and (host.deb or host.image):
        raise ConfigError('[mac]: deb and image are Linux deliveries')
    if not re.fullmatch(r'[0-9.]+:[0-9]{1,5}', host.listen):
        raise ConfigError(f'[{label}] listen: {host.listen!r} is not ADDRESS:PORT')
    if host.capture and not all(0 < value <= 4000 for value in vars(host.capture).values()):
        raise ConfigError(f'[{label}.capture]: width, height and fps 1 to 4000')
    return host


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
    unknown = set(data) - {'wled', 'leds', 'checks', 'mac', 'linux', 'tv', 'receiver', 'sweep', 'eyes'}
    if unknown:
        raise ConfigError(f'unknown sections {sorted(unknown)}')
    config = Config(
        wled=address(_table(data, 'wled', 'wled').get('address', ''), 'wled.address'),
        leds=_section(_table(data, 'leds', 'leds'), Leds, 'leds'),
        checks=_section(_table(data, 'checks', 'checks'), Checks, 'checks'),
        mac=_machine(data, Platform.MAC),
        linux=_machine(data, Platform.LINUX),
        tv=_section(_table(data, 'tv', 'tv'), Tv, 'tv'),
        receiver=_section(_table(data, 'receiver', 'receiver'), Receiver, 'receiver'),
        sweep=_section(_table(data, 'sweep', 'sweep'), Sweep, 'sweep'),
        eyes_portal=_table(data, 'eyes', 'eyes').get('portal', ''),
    )
    for name, count in config.leds.as_settings().items():
        if not 0 <= count <= 16384:
            raise ConfigError(f'leds.{name}: 0 to 16384')
    if not 0 < config.checks.fps_tolerance < 1:
        raise ConfigError('checks.fps_tolerance: between 0 and 1')
    _check_tv(config.tv)
    for name in ('url', 'wss_url'):
        value = getattr(config.receiver, name)
        if value and not _URL.fullmatch(value):
            raise ConfigError(f'receiver.{name}: {value!r} is not scheme://host[:port]')
    address(config.receiver.advertise, 'receiver.advertise')
    if config.eyes_portal and not _URL.fullmatch(config.eyes_portal):
        raise ConfigError('eyes.portal: http://host[:port]')
    for size in config.sweep.sizes:
        if not _SIZE.fullmatch(size):
            raise ConfigError(f'sweep.sizes: {size!r} is not WIDTHxHEIGHT')
    return config


def _check_tv(tv: Tv) -> None:
    """`[tv]`: the address, the ssh target (root@ADDRESS by default), the flavours."""
    address(tv.address, 'tv.address')
    if tv.address and not tv.ssh:
        tv.ssh = 'root@' + tv.address
    if tv.ssh:
        try:
            commands.ssh_target(tv.ssh)
        except commands.Refused as error:
            raise ConfigError(str(error)) from error
    tv.flavours = [_member(Flavour, flavour, f'tv.flavours: {flavour!r}') for flavour in tv.flavours]
    tv.restore_flavour = _member(RestoreFlavour, tv.restore_flavour, 'tv.restore_flavour: as-found, full, slim or none')


Member = TypeVar('Member', Flavour, RestoreFlavour)


def _member(kind: type[Member], value: str, refusal: str) -> Member:
    """`value` as a member of `kind`; `refusal` when it is none."""
    try:
        return kind(value)
    except ValueError:
        raise ConfigError(refusal) from None


def load(path: Path) -> Config:
    with path.open('rb') as file:
        try:
            return parse(tomllib.load(file))
        except tomllib.TOMLDecodeError as error:
            raise ConfigError(f'{path}: {error}') from error
