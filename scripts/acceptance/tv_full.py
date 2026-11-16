"""The full package's checks: the TV's own portal, capture, soaks, modes, sweep."""
import time

import modes
from plan import Failed
import prysm_api
from prysm_api import near
import tv
import web

SOAKS = [(640, 360, 30), (1280, 720, 30)]


def portal(ctx):
    return prysm_api.Prysm(ctx.config.tv.portal)


def wait_portal(ctx, seconds=40):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            return portal(ctx).state()
        except (web.Unreachable, RuntimeError):
            time.sleep(1)
    raise Failed(f'the TV portal {ctx.config.tv.portal} did not answer within {seconds} s')


def put_back(ctx):
    """The TV's settings as saved by the portal step (capture size and all)."""
    reply = portal(ctx).replace_settings(ctx.data['full_settings'])
    if reply.status != 200:
        raise Failed(f'the TV settings were not put back: HTTP {reply.status}', reply.json())


def start(ctx, seconds=30):
    prysm = portal(ctx)
    reply = prysm.set_target('processed')
    if reply.status != 204:
        raise Failed(f'target processed: HTTP {reply.status}', reply.json())
    state = prysm.wait(prysm_api.running, seconds)
    if state is None:
        raise Failed(f'no frames within {seconds} s: is video playing on the TV?', prysm.state())
    return state


def portal_step(ctx):
    state = wait_portal(ctx)
    ctx.data['full_settings'] = state['settings']
    path = ctx.save('tv-full-settings.json', state['settings'])
    return f'portal answers; settings saved to {path}', {'target': state['target'], 'session': state['session']}


def capture(ctx):
    state = start(ctx)
    luna = tv.wait_state(ctx, lambda s: s['capturing'], 10)
    if luna is None:
        raise Failed('getState does not say capturing', tv.get_state(ctx))
    return f'capturing at {state["input"]["fps"]} fps; portal {luna.get("portal")}', \
        {'portal': state['input'], 'luna': luna}


def _set_size(prysm, width, height, fps):
    reply = prysm.change_settings(lambda s: s['capture'].update(width=width, height=height, fps=fps))
    if reply.status != 200:
        raise Failed(f'capture {width}x{height}@{fps}: HTTP {reply.status}', reply.json())


def soak(width, height, fps):
    def run(ctx):
        prysm = portal(ctx)
        try:
            _set_size(prysm, width, height, fps)
            time.sleep(2)  # the session restarts with the new size
            start(ctx)
            result = tv.soak(ctx, prysm, ctx.config.tv.soak_seconds)
        finally:
            put_back(ctx)
        result['size'] = f'{width}x{height}@{fps}'
        if not near(result['fps_mean'], fps, ctx.config.desktop.fps_tolerance):
            raise Failed(f'{result["fps_mean"]} fps over the soak, expected {fps}', result)
        return tv.soak_summary(result), result
    return run


def capture_modes(ctx):
    reply = portal(ctx).modes({'kind': 'webos'})
    answer = reply.json()
    if reply.status != 200 or answer.get('status') != 'known':
        raise Failed(f'capture modes: HTTP {reply.status}', answer)
    return '; '.join(modes.describe(answer['modes'])), answer


def sweep(ctx):
    prysm = portal(ctx)
    rows = []
    try:
        for width, height, fps in ctx.config.sweep.pairs():
            def begin(w=width, h=height, f=fps):
                reply = prysm.change_settings(lambda s: s['capture'].update(width=w, height=h, fps=f))
                return reply if reply.status != 200 else prysm.set_target('processed')
            rows.append({'flavour': 'full', 'encoding': 'capture', 'width': width, 'height': height,
                         'requested_fps': fps, **tv.measure_row(ctx, prysm, begin)})
    finally:
        put_back(ctx)
    path = tv.write_table(ctx, 'sweep-full.csv', rows)
    return f'{len(rows)} rows in {path}', rows


def resumed(ctx):
    """After a reinstall or wake: lighting runs again (when it ran before)."""
    return portal(ctx).wait(prysm_api.running, 30) is not None
