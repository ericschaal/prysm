"""WLED's JSON API, and the snapshot that leaves WLED as it was found.

The snapshot holds what Prysm or the checks change: `on`, `bri`, `lor`,
`transition`, the active preset, and the configuration fields the
brightness check turns (`if.live.maxbri`). It is written to the run
directory before anything is changed, so `run.py restore-wled FILE`
can put it back after a hard kill.
"""
import json
import time

import web


class Wled:
    def __init__(self, address, timeout=4.0):
        if '/' in address or address.startswith('-'):
            raise ValueError(f'WLED address {address!r}: host[:port], no scheme')
        self.base = 'http://' + address
        self.timeout = timeout

    def get(self, path):
        reply = web.request('GET', self.base + path, timeout=self.timeout)
        if reply.status != 200:
            raise RuntimeError(f'WLED GET {path}: HTTP {reply.status}')
        return reply

    def si(self):
        return self.get('/json/si').json()

    def cfg_raw(self):
        return self.get('/json/cfg').body

    def post(self, path, body):
        reply = web.request('POST', self.base + path, body, timeout=self.timeout)
        if reply.status != 200:
            raise RuntimeError(f'WLED POST {path} {body}: HTTP {reply.status}')
        return reply.json()

    def post_state(self, body):
        return self.post('/json/state', body)

    def post_cfg(self, body):
        return self.post('/json/cfg', body)

    def wait(self, check, seconds, every=0.5):
        """The first `/json/si` for which `check` holds, else `None`."""
        deadline = time.monotonic() + seconds
        while True:
            si = self.si()
            if check(si):
                return si
            if time.monotonic() >= deadline:
                return None
            time.sleep(every)


def snapshot(wled):
    si = wled.si()
    cfg = json.loads(wled.cfg_raw())
    state = si['state']
    return {
        'state': {key: state[key] for key in ('on', 'bri', 'lor', 'transition', 'ps') if key in state},
        'cfg': {'maxbri': cfg.get('if', {}).get('live', {}).get('maxbri', False)},
        'info': {key: si['info'].get(key) for key in ('ver', 'name', 'live', 'lm', 'lip')},
        'leds': si['info'].get('leds', {}).get('count'),
    }


def restore(wled, saved):
    """Put WLED back: end any JSON live mode, the preset, then on/bri/lor/transition, then the configuration."""
    wled.post_state({'live': False})
    state = dict(saved['state'])
    preset = state.pop('ps', -1)
    if isinstance(preset, int) and preset > 0:
        wled.post_state({'ps': preset})
    wled.post_state(state)
    current = json.loads(wled.cfg_raw()).get('if', {}).get('live', {}).get('maxbri', False)
    if current != saved['cfg']['maxbri']:
        wled.post_cfg({'if': {'live': {'maxbri': saved['cfg']['maxbri']}}})
    return differences(wled, saved)


def gamma_fields(cfg):
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
    interfaces = cfg.get('if')
    live = interfaces.get('live') if isinstance(interfaces, dict) else None
    light = cfg.get('light', {})
    gc = light.get('gc', {}) if isinstance(light, dict) else None
    if not isinstance(live, dict):
        return {}, [f'if.live is missing or not an object: {refused}']
    if not isinstance(light, dict) or not isinstance(gc, dict):
        return {}, [f'light or light.gc is not an object: {refused}']
    found = {'if.live.no-gc': live.get('no-gc'), 'light.gc.col': gc.get('col'),
             'light.gc.bri': gc.get('bri'), 'light.gc.val': gc.get('val')}
    problems = []
    if 'no-gc' not in live:
        problems.append('if.live.no-gc is missing: Prysm would take realtime gamma correction as on')
    elif not isinstance(live['no-gc'], bool):
        problems.append(f'if.live.no-gc is {live["no-gc"]!r}, not true or false: {refused}')
    for name, fallback in (('col', 'take the colour gamma as unknown and send sRGB'),
                           ('bri', 'take the brightness gamma as off')):
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


def describe_gamma(found):
    """The gamma WLED applies to Prysm's colours, from `gamma_fields`, in words."""
    if found['if.live.no-gc']:
        colour = 'realtime gamma correction off (linear)'
    elif found['light.gc.col'] == 1:
        colour = 'colour gamma off'
    else:
        colour = f'colour gamma {found["light.gc.col"]}'
    bri = found['light.gc.bri']
    return colour + (', brightness gamma off' if bri == 1 else f', brightness gamma {bri}')


def differences(wled, saved):
    """What still differs from the snapshot (empty when restored)."""
    now = snapshot(wled)
    found = {}
    for key in ('on', 'bri', 'lor'):
        if key in saved['state'] and now['state'].get(key) != saved['state'][key]:
            found[key] = {'was': saved['state'][key], 'now': now['state'].get(key)}
    if now['cfg']['maxbri'] != saved['cfg']['maxbri']:
        found['maxbri'] = {'was': saved['cfg']['maxbri'], 'now': now['cfg']['maxbri']}
    return found
