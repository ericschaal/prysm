"""WLED's JSON API, and the snapshot that leaves WLED as it was found.

The snapshot holds what Prysm or the checks change: `on`, `bri`, `lor`,
`transition`, the active preset, and the configuration fields the
brightness check turns (`if.live.maxbri`). It is written to the run
directory before anything is changed, so `acceptance restore-wled FILE`
can put it back after a hard kill.

`/json/si` is typed here as the TypedDicts below (the fields the runner reads,
not validated). `/json/cfg` stays a JSON object: the checks read it as Prysm
does, field by field, and say what is missing or of another type.
"""

from collections.abc import Callable
import json
import time
from typing import TypedDict, cast

from prysm_acceptance import web
from prysm_acceptance.web import Json


class WledState(TypedDict, total=False):
    on: bool
    bri: int
    lor: int
    transition: int
    ps: int


class Leds(TypedDict, total=False):
    count: int


class Info(TypedDict, total=False):
    ver: str
    name: str
    live: bool
    lm: str
    lip: str
    leds: Leds


class Si(TypedDict):
    """`GET /json/si`."""

    state: WledState
    info: Info


class SavedCfg(TypedDict):
    maxbri: object


class SavedInfo(TypedDict):
    ver: str | None
    name: str | None
    live: bool | None
    lm: str | None
    lip: str | None


class Snapshot(TypedDict):
    """`snapshot`, as saved in `wled-snapshot.json`."""

    state: WledState
    cfg: SavedCfg
    info: SavedInfo
    leds: int | None


class Wled:
    def __init__(self, address: str, timeout: float = 4.0) -> None:
        if '/' in address or address.startswith('-'):
            raise ValueError(f'WLED address {address!r}: host[:port], no scheme')
        self.base = 'http://' + address
        self.timeout = timeout

    def get(self, path: str) -> web.Reply:
        reply = web.request('GET', self.base + path, timeout=self.timeout)
        if reply.status != 200:
            raise RuntimeError(f'WLED GET {path}: HTTP {reply.status}')
        return reply

    def si(self) -> Si:
        return cast('Si', self.get('/json/si').json())

    def cfg_raw(self) -> bytes:
        return self.get('/json/cfg').body

    def post(self, path: str, body: object) -> Json:
        reply = web.request('POST', self.base + path, body, timeout=self.timeout)
        if reply.status != 200:
            raise RuntimeError(f'WLED POST {path} {body}: HTTP {reply.status}')
        return reply.json()

    def post_state(self, body: object) -> Json:
        return self.post('/json/state', body)

    def post_cfg(self, body: object) -> Json:
        return self.post('/json/cfg', body)

    def wait(self, check: Callable[[Si], object], seconds: float, every: float = 0.5) -> Si | None:
        """The first `/json/si` for which `check` holds, else `None`."""
        deadline = time.monotonic() + seconds
        while True:
            si = self.si()
            if check(si):
                return si
            if time.monotonic() >= deadline:
                return None
            time.sleep(every)


def cfg(raw: bytes) -> dict[str, Json]:
    """`/json/cfg` as the JSON object it is."""
    return cast('dict[str, Json]', json.loads(raw))


def lookup(config: dict[str, Json], *path: str, default: Json = None) -> Json:
    """`config[a][b]...` along `path`, an absent object read as empty; `default` for an absent field.

    A member on the way that is not an object fails (AttributeError): WLED's layout moved."""
    value: Json = config
    for key in path[:-1]:
        value = cast('dict[str, Json]', value).get(key, {})
    return cast('dict[str, Json]', value).get(path[-1], default)


def snapshot(wled: Wled) -> Snapshot:
    si = wled.si()
    config = cfg(wled.cfg_raw())
    state = si['state']
    info = si['info']
    return {
        'state': cast(
            'WledState', {key: state[key] for key in ('on', 'bri', 'lor', 'transition', 'ps') if key in state}
        ),
        'cfg': {'maxbri': lookup(config, 'if', 'live', 'maxbri', default=False)},
        'info': {
            'ver': info.get('ver'),
            'name': info.get('name'),
            'live': info.get('live'),
            'lm': info.get('lm'),
            'lip': info.get('lip'),
        },
        'leds': info.get('leds', Leds()).get('count'),
    }


def restore(wled: Wled, saved: Snapshot) -> dict[str, dict[str, object]]:
    """Put WLED back: end any JSON live mode, the preset, then on/bri/lor/transition, then the configuration."""
    wled.post_state({'live': False})
    state = dict(saved['state'])
    preset = state.pop('ps', -1)
    if isinstance(preset, int) and preset > 0:
        wled.post_state({'ps': preset})
    wled.post_state(state)
    current = lookup(cfg(wled.cfg_raw()), 'if', 'live', 'maxbri', default=False)
    if current != saved['cfg']['maxbri']:
        wled.post_cfg({'if': {'live': {'maxbri': saved['cfg']['maxbri']}}})
    return differences(wled, saved)


def gamma_fields(config: dict[str, Json]) -> tuple[dict[str, Json], list[str]]:
    """The gamma fields Prysm reads from WLED's `/json/cfg`, checked as Prysm parses them.

    Mirrors `Config`, `Live`, `Light`, `GammaCorrection` and
    `From<Config> for Realtime` in adapters/output-wled/src/observe.rs and
    `Gamma::new` in adapters/output-wled/src/ddp.rs; keep them in step.
    Returns the raw fields and the problems, each naming the field and what
    Prysm would do. A missing field fails too, although Prysm then takes a
    default: it means WLED's layout moved. `light.gc.val` is evidence only;
    Prysm does not read it.
    """
    refused = 'Prysm would refuse the whole configuration read (no brightness, gamma unknown: sRGB)'
    interfaces = config.get('if')
    live = interfaces.get('live') if isinstance(interfaces, dict) else None
    light = config.get('light', {})
    gc = light.get('gc', {}) if isinstance(light, dict) else None
    if not isinstance(live, dict):
        return {}, [f'if.live is missing or not an object: {refused}']
    if not isinstance(light, dict) or not isinstance(gc, dict):
        return {}, [f'light or light.gc is not an object: {refused}']
    found: dict[str, Json] = {
        'if.live.no-gc': live.get('no-gc'),
        'light.gc.col': gc.get('col'),
        'light.gc.bri': gc.get('bri'),
        'light.gc.val': gc.get('val'),
    }
    problems: list[str] = []
    if 'no-gc' not in live:
        problems.append('if.live.no-gc is missing: Prysm would take realtime gamma correction as on')
    elif not isinstance(live['no-gc'], bool):
        problems.append(f'if.live.no-gc is {live["no-gc"]!r}, not true or false: {refused}')
    for name, fallback in (
        ('col', 'take the colour gamma as unknown and send sRGB'),
        ('bri', 'take the brightness gamma as off'),
    ):
        value = gc.get(name)
        field = f'light.gc.{name}'
        if value is None:
            # serde reads null as a missing value.
            problems.append(f'{field} is missing: Prysm would {fallback}')
        elif isinstance(value, bool) or not isinstance(value, (int, float)):
            problems.append(f'{field} is {value!r}, not a number: {refused}')
        elif not 0.1 <= value <= 3:
            problems.append(f'{field} is {value}, outside 0.1–3: Prysm would {fallback}')
    return found, problems


def describe_gamma(found: dict[str, Json]) -> str:
    """The gamma WLED applies to Prysm's colours, from `gamma_fields`, in words."""
    if found['if.live.no-gc']:
        colour = 'realtime gamma correction off (linear)'
    elif found['light.gc.col'] == 1:
        colour = 'colour gamma off'
    else:
        colour = f'colour gamma {found["light.gc.col"]}'
    bri = found['light.gc.bri']
    return colour + (', brightness gamma off' if bri == 1 else f', brightness gamma {bri}')


def differences(wled: Wled, saved: Snapshot) -> dict[str, dict[str, object]]:
    """What still differs from the snapshot (empty when restored)."""
    now = snapshot(wled)
    found: dict[str, dict[str, object]] = {}
    for key in ('on', 'bri', 'lor'):
        if key in saved['state'] and now['state'].get(key) != saved['state'][key]:
            found[key] = {'was': saved['state'][key], 'now': now['state'].get(key)}
    if now['cfg']['maxbri'] != saved['cfg']['maxbri']:
        found['maxbri'] = {'was': saved['cfg']['maxbri'], 'now': now['cfg']['maxbri']}
    return found
