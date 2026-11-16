"""Prysm's portal API (surfaces/web-portal/HTTP-CONTRACT.md), as the checks use it."""
import copy
import statistics
import time

import web

TARGETS = ('off', 'processed', 'device_native')


class Prysm:
    def __init__(self, base, timeout=20.0):
        self.base = base.rstrip('/')
        self.timeout = timeout

    def _url(self, path):
        return self.base + path

    def state(self):
        return web.get_json(self._url('/api/v1/state'))

    def set_target(self, target):
        """`PUT /api/v1/target`; the reply (204 once applied)."""
        if target not in TARGETS:
            raise ValueError(target)
        return web.request('PUT', self._url('/api/v1/target'), {'target': target}, timeout=self.timeout)

    def replace_settings(self, settings):
        """Save whole settings against the current revision; the reply."""
        revision = self.state()['revision']
        return web.request('PUT', self._url('/api/v1/settings'),
                           {'expected': revision, 'settings': settings}, timeout=self.timeout)

    def change_settings(self, change):
        """Apply `change(settings)` to a copy of the current settings and save it."""
        state = self.state()
        settings = copy.deepcopy(state['settings'])
        change(settings)
        return web.request('PUT', self._url('/api/v1/settings'),
                           {'expected': state['revision'], 'settings': settings}, timeout=self.timeout)

    def modes(self, capture):
        return web.request('POST', self._url('/api/v1/capture/modes'), capture, timeout=self.timeout)

    def candidates(self, section, kind):
        return web.get_json(self._url(f'/api/v1/candidates/{section}/{kind}'), timeout=self.timeout)

    def calibration(self, pattern):
        if pattern is None:
            return web.request('DELETE', self._url('/api/v1/calibration'), timeout=self.timeout)
        return web.request('PUT', self._url('/api/v1/calibration'),
                           {'pattern': pattern, 'uncorrected': False}, timeout=self.timeout)

    def colors(self, seconds=3.0):
        """The `ColorsEvent`s the colours stream sends within `seconds`."""
        return web.events(self._url('/api/v1/colors/stream'), seconds)

    def wait(self, check, seconds, every=0.5):
        """The first state for which `check(state)` holds; `None` after `seconds`."""
        deadline = time.monotonic() + seconds
        while True:
            state = self.state()
            if check(state):
                return state
            if time.monotonic() >= deadline:
                return None
            time.sleep(every)

    def sample(self, seconds, every=1.0):
        """`input` every `every` s for `seconds` (absent while no session runs)."""
        found = []
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            found.append(self.state().get('input'))
            time.sleep(every)
        return found


def session(state):
    return (state or {}).get('session', {}).get('state')


def running(state):
    return session(state) == 'running' and (state.get('input') or {}).get('receiving', False)


def rates(samples):
    """Mean and minimum input and sent rates of `sample`'s inputs; zeros when none ran."""
    inputs = [s for s in samples if s]
    if not inputs:
        return {'samples': len(samples), 'fps_mean': 0.0, 'fps_min': 0.0, 'sent_mean': 0.0, 'behind': []}
    fps = [s.get('fps', 0.0) for s in inputs]
    sent = [s.get('sent_fps', 0.0) for s in inputs]
    return {
        'samples': len(samples),
        'fps_mean': round(statistics.fmean(fps), 2),
        'fps_min': round(min(fps), 2),
        'sent_mean': round(statistics.fmean(sent), 2),
        'behind': sorted({s['behind'] for s in inputs if s.get('behind')}),
    }


def near(value, expected, tolerance):
    return abs(value - expected) <= expected * tolerance
