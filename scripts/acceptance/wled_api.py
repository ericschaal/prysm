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
