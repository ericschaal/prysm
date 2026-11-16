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

_ADDRESS = re.compile(r'[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?')
_SIZE = re.compile(r'([0-9]{1,4})x([0-9]{1,4})')
_URL = re.compile(r'https?://[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?/?')

# The TOML document as tomllib reads it. Every value is checked here before it
# becomes a typed field.
Toml: TypeAlias = Mapping[str, Any]


class ConfigError(ValueError):
    pass


class Preset(StrEnum):
    """The portal's size-and-rate presets (`modes.PRESETS`)."""

    BALANCED = 'balanced'
    SMOOTH = 'smooth'
    EFFICIENT = 'efficient'


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
class Desktop:
    headless: str = ''
    listen: str = '127.0.0.1:18787'
    seconds: int = 10
    fps_tolerance: float = 0.15
    device: str = ''
    preset: Preset = Preset.BALANCED
    display_port: int = 18790


@dataclass
class Tv:
    address: str = ''
    ssh: str = ''
    ares_device: str = ''
    flavours: list[Flavour] = field(default_factory=lambda: [Flavour.FULL, Flavour.SLIM])
    restore_flavour: RestoreFlavour = RestoreFlavour.AS_FOUND
    soak_seconds: int = 90
    ipk_full: str = ''
    ipk_slim: str = ''

    @property
    def portal(self) -> str:
        return f'http://{self.address}:8787'

    def ipk(self, flavour: Flavour) -> str:
        """The configured package of `flavour` (`ipk_full`, `ipk_slim`); empty: build it."""
        return self.ipk_full if flavour == Flavour.FULL else self.ipk_slim


@dataclass
class Receiver:
    url: str = ''
    listen: str = '0.0.0.0:8788'
    advertise: str = ''
    wss_url: str = ''
    mismatch_port: int = 18791


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
class ChromecastFeed:
    """`[feed.chromecast]`: the Chromecast whose HDMI output the capture card records."""

    adb_serial: str = ''
    serve_port: int = 18792
    settle_seconds: float = 5.0


@dataclass
class Config:
    wled: str = ''
    leds: Leds = field(default_factory=Leds)
    desktop: Desktop = field(default_factory=Desktop)
    tv: Tv = field(default_factory=Tv)
    receiver: Receiver = field(default_factory=Receiver)
    sweep: Sweep = field(default_factory=Sweep)
    feed: ChromecastFeed | None = None  # None: no controlled feed into the capture card
    eyes_portal: str = ''


Section = TypeVar('Section', Leds, Desktop, Tv, Receiver, Sweep, ChromecastFeed)


def _section(data: Toml, name: str, kind: type[Section], label: str | None = None) -> Section:
    label = label or name
    table = data.get(name, {})
    if not isinstance(table, dict):
        raise ConfigError(f'[{label}] must be a table')
    values = cast('dict[str, Any]', table)
    known = {item.name for item in fields(kind)}
    unknown = set(values) - known
    if unknown:
        raise ConfigError(f'[{label}]: unknown keys {sorted(unknown)}')
    defaults = kind()
    for key, value in values.items():
        default: object = getattr(defaults, key)
        expected = type(default)
        if issubclass(expected, StrEnum):
            expected = str  # checked against its members in `parse`
        # An integer is a float too; the value itself is kept as given.
        checked = float(value) if expected is float and isinstance(value, int) else value
        if not isinstance(checked, expected) or isinstance(checked, bool) != (expected is bool):
            raise ConfigError(f'[{label}] {key}: expected {expected.__name__}')
    return kind(**values)


def _feed(data: Toml) -> ChromecastFeed | None:
    """`[feed.chromecast]`, or None without a `[feed]`."""
    table = data.get('feed', {})
    if not isinstance(table, dict):
        raise ConfigError('[feed] must be a table')
    feeds = cast('dict[str, object]', table)
    unknown = set(feeds) - {'chromecast'}
    if unknown:
        raise ConfigError(f'[feed]: unknown feeds {sorted(unknown)} (supported: chromecast)')
    if 'chromecast' not in feeds:
        return None
    chromecast = _section(feeds, 'chromecast', ChromecastFeed, 'feed.chromecast')
    if not chromecast.adb_serial:
        raise ConfigError('feed.chromechromecast.adb_serial is required (the serial adb devices lists)')
    try:
        commands.adb_serial(chromecast.adb_serial)
    except commands.Refused as error:
        raise ConfigError(str(error)) from error
    if not 1 <= chromecast.serve_port <= 65535:
        raise ConfigError('feed.chromechromecast.serve_port: 1 to 65535')
    if not 0 <= chromecast.settle_seconds <= 60:
        raise ConfigError('feed.chromechromecast.settle_seconds: 0 to 60')
    return chromecast


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
    unknown = set(data) - {'wled', 'leds', 'desktop', 'tv', 'receiver', 'sweep', 'eyes', 'feed'}
    if unknown:
        raise ConfigError(f'unknown sections {sorted(unknown)}')
    wled = data.get('wled', {})
    config = Config(
        wled=address(wled.get('address', ''), 'wled.address'),
        leds=_section(data, 'leds', Leds),
        desktop=_section(data, 'desktop', Desktop),
        tv=_section(data, 'tv', Tv),
        receiver=_section(data, 'receiver', Receiver),
        sweep=_section(data, 'sweep', Sweep),
        feed=_feed(data),
        eyes_portal=data.get('eyes', {}).get('portal', ''),
    )
    for name, count in config.leds.as_settings().items():
        if not 0 <= count <= 16384:
            raise ConfigError(f'leds.{name}: 0 to 16384')
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
    config.desktop.preset = _member(Preset, config.desktop.preset, 'desktop.preset: balanced, smooth or efficient')
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


Member = TypeVar('Member', Flavour, Preset, RestoreFlavour)


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
