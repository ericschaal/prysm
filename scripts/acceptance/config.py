"""The runner's configuration: a TOML file (see config.example.toml), checked into typed values."""
from dataclasses import dataclass, field
import re
import tomllib

import commands

_ADDRESS = re.compile(r'[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?')
_SIZE = re.compile(r'([0-9]{1,4})x([0-9]{1,4})')
_URL = re.compile(r'https?://[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?/?')


class ConfigError(ValueError):
    pass


@dataclass
class Leds:
    top: int = 0
    right: int = 0
    bottom: int = 0
    left: int = 0

    def as_settings(self):
        return {'top': self.top, 'right': self.right, 'bottom': self.bottom, 'left': self.left}


@dataclass
class Desktop:
    headless: str = ''
    listen: str = '127.0.0.1:18787'
    seconds: int = 10
    fps_tolerance: float = 0.15
    device: str = ''
    preset: str = 'balanced'
    display_port: int = 18790


@dataclass
class Tv:
    address: str = ''
    ssh: str = ''
    ares_device: str = ''
    flavours: list = field(default_factory=lambda: ['full', 'slim'])
    restore_flavour: str = 'as-found'
    soak_seconds: int = 90
    ipk_full: str = ''
    ipk_slim: str = ''

    @property
    def portal(self):
        return f'http://{self.address}:8787'


@dataclass
class Receiver:
    url: str = ''
    listen: str = '0.0.0.0:8788'
    advertise: str = ''
    wss_url: str = ''
    mismatch_port: int = 18791


@dataclass
class Sweep:
    sizes: list = field(default_factory=lambda: ['320x180', '640x360', '960x540', '1280x720'])
    rates: list = field(default_factory=lambda: [30, 60])
    seconds: int = 10

    def pairs(self):
        found = []
        for size in self.sizes:
            match = _SIZE.fullmatch(size)
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


def _section(data, name, kind, label=None):
    label = label or name
    values = data.get(name, {})
    if not isinstance(values, dict):
        raise ConfigError(f'[{label}] must be a table')
    known = set(kind.__dataclass_fields__)
    unknown = set(values) - known
    if unknown:
        raise ConfigError(f'[{label}]: unknown keys {sorted(unknown)}')
    defaults = kind()
    for key, value in values.items():
        expected = type(getattr(defaults, key))
        if expected is float and isinstance(value, int):
            value = float(value)
        if not isinstance(value, expected) or isinstance(value, bool) != (expected is bool):
            raise ConfigError(f'[{label}] {key}: expected {expected.__name__}')
    return kind(**values)


def _feed(data):
    """`[feed.chromecast]`, or None without a `[feed]`."""
    feeds = data.get('feed', {})
    if not isinstance(feeds, dict):
        raise ConfigError('[feed] must be a table')
    unknown = set(feeds) - {'chromecast'}
    if unknown:
        raise ConfigError(f'[feed]: unknown feeds {sorted(unknown)} (supported: chromecast)')
    if 'chromecast' not in feeds:
        return None
    cast = _section(feeds, 'chromecast', ChromecastFeed, 'feed.chromecast')
    if not cast.adb_serial:
        raise ConfigError('feed.chromecast.adb_serial is required (the serial adb devices lists)')
    try:
        commands.adb_serial(cast.adb_serial)
    except commands.Refused as error:
        raise ConfigError(str(error)) from error
    if not 1 <= cast.serve_port <= 65535:
        raise ConfigError('feed.chromecast.serve_port: 1 to 65535')
    if not 0 <= cast.settle_seconds <= 60:
        raise ConfigError('feed.chromecast.settle_seconds: 0 to 60')
    return cast


def _address(value, name, required=False):
    if not value:
        if required:
            raise ConfigError(f'{name} is required')
        return value
    if not _ADDRESS.fullmatch(value):
        raise ConfigError(f'{name}: {value!r} is not host[:port]')
    return value


def parse(data):
    unknown = set(data) - {'wled', 'leds', 'desktop', 'tv', 'receiver', 'sweep', 'eyes', 'feed'}
    if unknown:
        raise ConfigError(f'unknown sections {sorted(unknown)}')
    wled = data.get('wled', {})
    config = Config(
        wled=_address(wled.get('address', ''), 'wled.address'),
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
    _address(config.tv.address, 'tv.address')
    if config.tv.address and not config.tv.ssh:
        config.tv.ssh = 'root@' + config.tv.address
    if config.tv.ssh:
        try:
            commands.ssh_target(config.tv.ssh)
        except commands.Refused as error:
            raise ConfigError(str(error)) from error
    for flavour in config.tv.flavours:
        if flavour not in ('full', 'slim'):
            raise ConfigError(f'tv.flavours: {flavour!r}')
    if config.tv.restore_flavour not in ('as-found', 'full', 'slim', 'none'):
        raise ConfigError('tv.restore_flavour: as-found, full, slim or none')
    for name in ('url', 'wss_url'):
        value = getattr(config.receiver, name)
        if value and not _URL.fullmatch(value):
            raise ConfigError(f'receiver.{name}: {value!r} is not scheme://host[:port]')
    _address(config.receiver.advertise, 'receiver.advertise')
    if config.eyes_portal and not _URL.fullmatch(config.eyes_portal):
        raise ConfigError('eyes.portal: http://host[:port]')
    for size in config.sweep.sizes:
        if not _SIZE.fullmatch(size):
            raise ConfigError(f'sweep.sizes: {size!r} is not WIDTHxHEIGHT')
    if config.desktop.preset not in ('balanced', 'smooth', 'efficient'):
        raise ConfigError('desktop.preset: balanced, smooth or efficient')
    return config


def load(path):
    with open(path, 'rb') as file:
        try:
            return parse(tomllib.load(file))
        except tomllib.TOMLDecodeError as error:
            raise ConfigError(f'{path}: {error}') from error
