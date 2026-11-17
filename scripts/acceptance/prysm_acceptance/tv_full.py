"""The full package's checks: the TV's own portal, capture, soaks, modes, sweep."""

from collections.abc import Callable
import time

from prysm_acceptance import prysm_api, tv, web
from prysm_acceptance.checks import Failed
from prysm_acceptance.plan import Bench, Context
from prysm_acceptance.prysm_api import Settings, Target, near, section
from prysm_acceptance.stage_data import TvData

Ctx = Context[TvData]

SOAKS = [(640, 360, 30), (1280, 720, 30)]


def portal(ctx: Bench) -> prysm_api.Prysm:
    """The TV's portal at the address `getState` reports (`portal`, once the full worker runs)."""
    url = (tv.get_state(ctx) or tv.LunaReply()).get('portal')
    if not url:
        raise Failed('getState reports no portal address: is the full worker running?')
    return prysm_api.Prysm(url)


def wait_portal(ctx: Bench, seconds: float = 40) -> prysm_api.State:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            return portal(ctx).state()
        except (Failed, web.Unreachable, RuntimeError):
            time.sleep(1)
    raise Failed(f'the TV portal (getState portal) did not answer within {seconds} s')


def put_back(ctx: Ctx) -> None:
    """The TV's settings as saved by the portal step (capture size and all)."""
    reply = portal(ctx).replace_settings(ctx.data['full_settings'])
    if reply.status != 200:
        raise Failed(f'the TV settings were not put back: HTTP {reply.status}', reply.json())


def start(ctx: Bench, seconds: float = 30) -> prysm_api.State:
    prysm = portal(ctx)
    reply = prysm.set_target(Target.PROCESSED)
    if reply.status != 204:
        raise Failed(f'target processed: HTTP {reply.status}', reply.json())
    state = prysm.wait(prysm_api.running, seconds)
    if state is None:
        raise Failed(f'no frames within {seconds} s: is video playing on the TV?', prysm.state())
    return state


def portal_step(ctx: Ctx) -> tuple[str, object]:
    state = wait_portal(ctx)
    ctx.data['full_settings'] = state['settings']
    path = ctx.save('tv-full-settings.json', state['settings'])
    return f'portal answers; settings saved to {path}', {'target': state['target'], 'session': state['session']}


def capture(ctx: Ctx) -> tuple[str, object]:
    state = start(ctx)
    luna = tv.wait_state(ctx, lambda s: s['capturing'], 10)
    if luna is None:
        raise Failed('getState does not say capturing', tv.get_state(ctx))
    return f'capturing at {(state["input"] or {})["fps"]} fps; portal {luna.get("portal")}', {
        'portal': state['input'],
        'luna': luna,
    }


def _size(width: int, height: int, fps: int) -> Callable[[Settings], None]:
    """A settings change to this capture size and rate."""
    return lambda settings: section(settings, 'capture').update(width=width, height=height, fps=fps)


def _set_size(prysm: prysm_api.Prysm, width: int, height: int, fps: int) -> None:
    reply = prysm.change_settings(_size(width, height, fps))
    if reply.status != 200:
        raise Failed(f'capture {width}x{height}@{fps}: HTTP {reply.status}', reply.json())


def soak(width: int, height: int, fps: int) -> Callable[[Ctx], tuple[str, object]]:
    def run(ctx: Ctx) -> tuple[str, object]:
        prysm = portal(ctx)
        try:
            _set_size(prysm, width, height, fps)
            time.sleep(2)  # the session restarts with the new size
            start(ctx)
            result = tv.soak(ctx, prysm, ctx.config.tv.soak_seconds)
        finally:
            put_back(ctx)
        result['size'] = f'{width}x{height}@{fps}'
        if not near(result['fps_mean'], fps, ctx.config.checks.fps_tolerance):
            raise Failed(f'{result["fps_mean"]} fps over the soak, expected {fps}', result)
        return tv.soak_summary(result), result

    return run


def capture_modes(ctx: Ctx) -> tuple[str, object]:
    reply = portal(ctx).modes({'kind': 'webos'})
    answer = prysm_api.modes_answer(reply)
    if reply.status != 200 or answer.get('status') != 'known':
        raise Failed(f'capture modes: HTTP {reply.status}', answer)
    return '; '.join(prysm_api.describe_modes(answer['modes'])), answer


def sweep(ctx: Ctx) -> tuple[str, object]:
    prysm = portal(ctx)
    rows: list[tv.Row] = []
    try:
        for width, height, fps in ctx.config.sweep.pairs():

            def begin(w: int = width, h: int = height, f: int = fps) -> web.Reply:
                reply = prysm.change_settings(_size(w, h, f))
                return reply if reply.status != 200 else prysm.set_target(Target.PROCESSED)

            rows.append(
                {
                    'flavour': 'full',
                    'encoding': 'capture',
                    'width': width,
                    'height': height,
                    'requested_fps': fps,
                    **tv.measure_row(ctx, prysm, begin),
                }
            )
    finally:
        put_back(ctx)
    path = tv.write_table(ctx, 'sweep-full.csv', rows)
    return f'{len(rows)} rows in {path}', rows


def resumed(ctx: Bench) -> bool:
    """After a reinstall or wake: lighting runs again (when it ran before)."""
    return portal(ctx).wait(prysm_api.running, 30) is not None
