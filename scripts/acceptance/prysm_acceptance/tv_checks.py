"""The checks any TV cell runs, through `tv.Tv` and Prysm's HTTP API; a TV's cells module (`tv_cells`) takes
them with its fixtures and gives each its plan for that TV (`planned`).

Every check that measures runs in a state of the TV it declares (`Shown`):
the runner sets it, confirms it from the TV itself, and only then measures
(`in_state`). VIDEO: the clip cast to the TV's Cast receiver with a known
overlay over its bottom, video available, the frames of the size asked.
UI: the cast stopped, Prysm's own app in front, video unavailable, the frames
of the TV's UI size. A state not reached fails the check, saying which part.
The checks are taken in this module's order, the video ones first, so the
clip is cast once and stopped once.
"""

from collections.abc import Callable, Generator
import contextlib
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
import time
from typing import TypedDict, TypeVar

import pytest

from prysm_acceptance import edges_clip, feed_cast, host_cells, prysm_api, tv
from prysm_acceptance.bench import only
from prysm_acceptance.checks import Answer, Bench, Failed, Skipped, ask_person, tell_person
from prysm_acceptance.config import Config
from prysm_acceptance.host import Host, Sender, shown
from prysm_acceptance.host_cells import Clip, Record, screen_sync
from prysm_acceptance.options import Flavour, TvCell
from prysm_acceptance.prysm_api import Prysm, SessionState, Target, near, rates
from prysm_acceptance.tv import Encoding, Power, Tv, Video
from prysm_acceptance.web import JsonObject
from prysm_acceptance.wled_api import Wled

# The capture every cell starts with, and the soaks (width, height, fps): the full package captures at each
# size, the slim package streams hardware H.264.
CAPTURE = (640, 360, 30)
SOAKS = {Flavour.FULL: [(640, 360, 30), (1280, 720, 30)], Flavour.SLIM: [(640, 360, 30)]}
# The TV's UI capture is at most this size, whatever was asked (crates/adapters/capture-webos settings.rs `ui_size`).
UI_LIMIT = (320, 180)
# How long the TV may take to report a state the runner set.
CONFIRM_SECONDS = 20
# How long the check without frames watches that none come and nothing fails.
QUIET_SECONDS = 10
# How long the TV may still report active after a person said its screen is off.
STANDBY_SECONDS = 60

TvPlan = Callable[[Tv, TvCell, Config], list[str]]
Check = TypeVar('Check', bound=Callable[..., object])
# Each check's plan for the dry run, given the TV it runs on (`planned`).
PLANS: dict[Callable[..., object], TvPlan] = {}


def planned(does: TvPlan) -> Callable[[Check], Check]:
    """A check's commands and actions for the dry run, as lines for the TV, the cell and the configuration. The
    cells module that takes the check records them with its own TV (`bench.Planner`)."""

    def record(check: Check) -> Check:
        PLANS[check] = does
        return check

    return record


@dataclass(frozen=True)
class Lit:
    """What lights WLED in a TV cell, for the shared checks and the failure report: the address WLED should see
    and what to fetch (the TV's recent log from getState, and a receiver's own)."""

    tv: Tv
    address: str
    out_dir: Path
    receiver: Host | None = None

    def sender(self) -> str:
        return self.address

    def fetch(self) -> None:
        state = self.tv.get_state() or tv.State()
        (self.out_dir / 'tv-worker.log').write_text(''.join(line + '\n' for line in state.get('log', [])))
        if self.receiver is not None:
            self.receiver.fetch()


class Shown(StrEnum):
    """What the TV shows while a check measures: the state the runner sets and confirms."""

    VIDEO = 'video'  # the clip cast and playing with its overlay: the Cast receiver in front, video available
    UI = 'ui'  # the cast stopped, Prysm's own app in front: video unavailable, the TV's UI captured


def wanted_on(found: Tv, wanted: Shown) -> tuple[Video, str]:
    """The video the TV must report in `wanted` and the app in front."""
    if wanted == Shown.VIDEO:
        return Video.AVAILABLE, found.cast_app
    return Video.UNAVAILABLE, found.prysm_app


class Reached(TypedDict):
    """A state as the TV confirmed it, each check's evidence: getState's power and video, the app in front,
    and the size of the frames that arrive (None: none arrive, or not measured)."""

    shown: Shown
    power: Power | None
    video: Video | None
    foreground: str
    size: str | None


class Screen:
    """What the TV shows: `show` sets a state, `confirm` waits until the TV reports it.

    VIDEO casts the clip with its overlay (`feed_cast.TvCast`) unless it plays, after closing Prysm's app if the
    runner launched it; the TV leaves its screen saver for the cast. UI stops the cast and launches Prysm's app.
    A cast that cannot reach PLAYING (also after the person was asked to accept it) is tried once: every later
    VIDEO state fails at once with its reason. `close` stops the cast and closes Prysm's app."""

    def __init__(self, found: Tv, bench: Bench, cast_feed: feed_cast.TvCast, clip: Clip) -> None:
        self.tv = found
        self.bench = bench
        self.feed = cast_feed
        self.clip = clip
        self.casting = False
        self.launched = False
        self.not_cast: str | None = None

    def show(self, wanted: Shown) -> None:
        if wanted == Shown.VIDEO:
            if self.launched:
                self.tv.close_app()
                self.launched = False
            if not self.casting:
                self._cast()
            return
        if self.casting:
            self.casting = False
            host_cells.stop_feed(self.bench)
        self.tv.launch_app()
        self.launched = True

    def _cast(self) -> None:
        if self.not_cast:
            raise Failed(f'the video state is not reached: {self.not_cast}')
        try:
            detail, evidence = self.feed.show(self.bench, str(self.clip.path))
        except (Failed, Skipped) as error:
            self.not_cast = f'the clip was not cast: {error}'
            self.bench.save('feed.json', {'failed': str(error), 'evidence': error.evidence})
            host_cells.stop_feed(self.bench)
            raise Failed(f'the video state is not reached: {self.not_cast}', error.evidence) from error
        self.casting = True
        self.bench.save('feed.json', {'detail': detail, 'evidence': evidence})

    def confirm(self, wanted: Shown) -> Reached:
        """`wanted` as the TV reports it within `CONFIRM_SECONDS`: the power active, the video exactly available
        or unavailable (unknown is not reached), the state's app in front; for VIDEO, once the player's own
        overlay has cleared, the clip still PLAYING. Else Failed, naming what was not reached."""
        video, app = wanted_on(self.tv, wanted)
        deadline = time.monotonic() + CONFIRM_SECONDS
        while True:
            state = self.tv.get_state() or tv.State()
            reached: Reached = {
                'shown': wanted,
                'power': state.get('power'),
                'video': state.get('video'),
                'foreground': self.tv.foreground(),
                'size': None,
            }
            missing = [
                *([f'power {reached["power"]}, not {Power.ACTIVE}'] if reached['power'] != Power.ACTIVE else []),
                *([f'video {reached["video"]}, not {video}'] if reached['video'] != video else []),
                *([f'{reached["foreground"] or "no app"} in front, not {app}'] if reached['foreground'] != app else []),
            ]
            if not missing:
                break
            if time.monotonic() >= deadline:
                raise Failed(f'the {wanted} state is not reached: {"; ".join(missing)}', reached)
            time.sleep(1)
        if wanted == Shown.VIDEO:
            with stated(reached):
                self.feed.settled()
        return reached

    def close(self) -> list[str]:
        """The cast stopped and Prysm's app closed, each also after the other failed; what failed. What is in
        front then is saved (tv-end.json)."""
        failures: list[str] = []
        try:
            host_cells.stop_feed(self.bench)
        except Failed as error:
            failures.append(str(error))
        if self.launched:
            try:
                self.tv.close_app()
            except Failed as error:
                failures.append(f"Prysm's app not closed: {error}")
        state = self.tv.get_state() or tv.State()
        self.bench.save('tv-end.json', {'foreground': self.tv.foreground(), 'power': state.get('power')})
        return failures


@contextlib.contextmanager
def stated(reached: Reached) -> Generator[None, None, None]:
    """A check's failure carries the state it measured in."""
    try:
        yield
    except Failed as error:
        raise Failed(str(error), {'state': reached, 'evidence': error.evidence}) from error


def described(reached: Reached) -> str:
    return (
        f'{reached["shown"]}: power {reached["power"]}, video {reached["video"]}, {reached["foreground"]} in front'
        + (f', {reached["size"]} frames' if reached['size'] else '')
    )


def _sized(wanted: Shown, size: str | None, section: JsonObject) -> str | None:
    """Why frames of `size` are not those of `wanted`: exactly the size asked (VIDEO), within `UI_LIMIT` (UI)."""
    if size is None:
        return 'no frames arrive'
    width, _, height = size.partition('x')
    if wanted == Shown.VIDEO:
        asked = f'{section["width"]}x{section["height"]}'
        return None if size == asked else f'frames of {size} arrive, not the {asked} asked'
    limit = UI_LIMIT
    within = width.isdigit() and height.isdigit() and 0 < int(width) <= limit[0] and 0 < int(height) <= limit[1]
    return None if within else f'frames of {size} arrive, not the UI size (at most {limit[0]}x{limit[1]})'


def in_state(screen: Screen, wanted: Shown, prysm: Prysm, section: JsonObject) -> Reached:
    """The TV set to `wanted` and confirmed (`Screen.confirm`), Prysm capturing `section` with Screen sync
    running with frames, and those frames of the state's size (`Prysm.source_size`); else Failed."""
    screen.show(wanted)
    reached = screen.confirm(wanted)
    with stated(reached):
        ask_for(prysm, section)
    reached['size'] = prysm.source_size()
    wrong = _sized(wanted, reached['size'], section)
    if wrong:
        raise Failed(f'the {wanted} state is not reached: {wrong}', reached)
    return reached


def capture(
    flavour: Flavour,
    width: int,
    height: int,
    fps: int,
    include_ui: bool = False,
    encoding: Encoding = Encoding.H264_HARDWARE,
) -> JsonObject:
    """The capture section: the TV's own (full), or the receiver's stream from the TV (slim). `include_ui`: the
    TV's menus and overlays blended over the video, or, without video, the TV's UI alone; without it and without
    video there are no frames."""
    size: JsonObject = {'width': width, 'height': height, 'fps': fps, 'include_ui': include_ui}
    if flavour == Flavour.FULL:
        return {'kind': 'webos', **size}
    return {'kind': 'remote', **size, 'encoding': encoding}


def _capture_saved(prysm: Prysm, section: JsonObject) -> None:
    """Save `section` as the capture; fails unless it is accepted."""
    reply = prysm.change_settings(lambda settings: settings.__setitem__('capture', section))
    if reply.status != 200:
        raise Failed(f'capture {section}: HTTP {reply.status}', reply.json())
    time.sleep(2)  # the session restarts with the new capture


def ask_for(prysm: Prysm, section: JsonObject) -> None:
    """Save `section` as the capture and light WLED (Screen sync); fails unless both are accepted and frames
    come within 30 s."""
    _capture_saved(prysm, section)
    screen_sync(prysm, seconds=30)


def state_plan(found: Tv, wanted: Shown, config: Config, include_ui: bool = False) -> list[str]:
    """`in_state`, for the dry run."""
    video, app = wanted_on(found, wanted)
    width, height, fps = CAPTURE
    sets = (
        "unless the clip plays: Prysm's app closed if the runner launched it, the clip cast (screen)"
        if wanted == Shown.VIDEO
        else f'the cast stopped if it plays, then $ {found.launch_command().display()}'
    )
    size = (
        'exactly the size asked' if wanted == Shown.VIDEO else f"at most {UI_LIMIT[0]}x{UI_LIMIT[1]}, the TV's UI size"
    )
    return [
        f'the {wanted} state: {sets}; then every second for up to {CONFIRM_SECONDS} s:',
        *shown(found.state_command(), found.foreground_command()),
        f'until power {Power.ACTIVE}, video {video}, {app} in front'
        + (
            f"; the player's overlay waited out ({config.tv.feed.settle_seconds:g} s after PLAYING)"
            if wanted == Shown.VIDEO
            else ''
        ),
        f'PUT /api/v1/settings capture {width}x{height}@{fps} ' + ('with' if include_ui else 'without') + ' the UI, '
        'PUT /api/v1/target screen_sync, running with frames within 30 s',
        f'GET /api/v1/preview/stream: the frames that arrive are {size}',
        '(any part not reached fails the check, saying which)',
    ]


# ---- the checks, in order: the video state first, then the UI's -------------------


def _judged_plan(found: Tv, config: Config, include_ui: bool) -> list[str]:
    return [
        *state_plan(found, Shown.VIDEO, config, include_ui),
        f'GET /api/v1/state every second for {config.checks.seconds} s',
        f'GET /api/v1/colors/stream for {edges_clip.CHECK_SECONDS:g} s',
    ]


def _video_judged(cell: TvCell, config: Config, screen: Screen, prysm: Prysm, include_ui: bool, record: Record) -> None:
    """Check 1 and 2: the video state, the capture with or without the UI, the rate, the colours judged."""
    reached = in_state(screen, Shown.VIDEO, prysm, capture(cell.flavour, *CAPTURE, include_ui=include_ui))
    with stated(reached):
        measured = host_cells.frame_rate(prysm, config)
        seconds = edges_clip.CHECK_SECONDS
        decoder = edges_clip.decode_overlaid if include_ui else edges_clip.decode
        detail, judged = edges_clip.judge(prysm.colors(seconds, timed=True), seconds, decoder)
    record(
        f'{detail}; {host_cells.rate_detail(measured)}; {described(reached)}',
        {'state': reached, 'rates': measured, 'colours': judged},
    )


@planned(does=lambda found, cell, config: _judged_plan(found, config, include_ui=False))
def test_video_only(cell: TvCell, config: Config, screen: Screen, prysm: Prysm, record: Record) -> None:
    """Video without the UI: the clip plays with the overlay over its bottom, the capture leaves the UI out.

    The frames arrive at the size asked, at the rate within the tolerance, and the colours follow the clip on
    every edge (the host cells' colour judgement): the overlay is not in the picture."""
    _video_judged(cell, config, screen, prysm, False, record)


@planned(does=lambda found, cell, config: _judged_plan(found, config, include_ui=True))
def test_video_with_overlay(cell: TvCell, config: Config, screen: Screen, prysm: Prysm, record: Record) -> None:
    """Video with the UI: the same state, only the capture now includes the UI.

    The same size and rate; the colours still follow the clip on the top edge and the sides' upper halves, and
    the bottom edge shows the overlay (white) at every step: the overlay is in the picture where it is, and
    nowhere else."""
    _video_judged(cell, config, screen, prysm, True, record)


def video_section(cell: TvCell) -> JsonObject:
    return capture(cell.flavour, *CAPTURE)


@planned(
    does=lambda found, cell, config: [
        *state_plan(found, Shown.VIDEO, config),
        f'GET http://{config.wled}/json/si',
    ]
)
def test_wled_live(cell: TvCell, screen: Screen, prysm: Prysm, wled: Wled, running: Sender, record: Record) -> None:
    """In the video state, Screen sync lights WLED: live DDP from the expected sender (the TV, or the slim
    package's receiver), no device.issue (the host cells' check)."""
    reached = in_state(screen, Shown.VIDEO, prysm, video_section(cell))
    with stated(reached):
        si = host_cells.live_from_host(prysm, wled, running)
    record(f'live DDP from {si["info"]["lip"]}; {described(reached)}', {'state': reached, 'wled': si['info']})


@planned(
    does=lambda found, cell, config: [
        *state_plan(found, Shown.VIDEO, config),
        f'GET /api/v1/state every second for {config.checks.seconds} s',
    ]
)
def test_fps(cell: TvCell, config: Config, screen: Screen, prysm: Prysm, record: Record) -> None:
    """In the video state, with the frames at the size asked: the input's fps within the tolerance of the
    capture's, sent fps alike, never behind (the host cells' check)."""
    reached = in_state(screen, Shown.VIDEO, prysm, video_section(cell))
    with stated(reached):
        measured = host_cells.frame_rate(prysm, config)
    record(f'{host_cells.rate_detail(measured)}; {described(reached)}', {'state': reached, 'rates': measured})


@planned(does=lambda found, cell, config: [*state_plan(found, Shown.VIDEO, config), *shown(found.state_command())])
def test_capturing(cell: TvCell, screen: Screen, prysm: Prysm, record: Record) -> None:
    """In the video state, the TV says it captures: getState capturing with a frame rate."""
    reached = in_state(screen, Shown.VIDEO, prysm, video_section(cell))
    state = tv.wait_state(screen.tv.get_state, lambda s: tv.capturing(s) and (tv.fps(s) or 0) > 0, 10)
    if state is None:
        raise Failed('getState does not say capturing', {'state': reached, 'tv': screen.tv.get_state()})
    record(f'capturing at {tv.fps(state)} fps; {described(reached)}', {'state': reached, 'tv': state})


@only(Flavour.FULL)
@planned(
    does=lambda found, cell, config: [
        *state_plan(found, Shown.VIDEO, config),
        'POST /api/v1/capture/modes {"kind":"webos"}',
    ]
)
def test_capture_modes(cell: TvCell, screen: Screen, prysm: Prysm, record: Record) -> None:
    """The full package, in the video state: the TV's capture modes are known."""
    reached = in_state(screen, Shown.VIDEO, prysm, video_section(cell))
    reply = prysm.modes({'kind': 'webos'})
    answer = prysm_api.modes_answer(reply)
    if reply.status != 200 or answer.get('status') != 'known':
        raise Failed(f'capture modes: HTTP {reply.status}', {'state': reached, 'answer': answer})
    record('; '.join(prysm_api.describe_modes(answer['modes'])), {'state': reached, 'answer': answer})


@only(Flavour.SLIM)
@planned(
    does=lambda found, cell, config: [
        *state_plan(found, Shown.VIDEO, config),
        'PUT /api/v1/target off',
        'POST /api/v1/capture/modes {"kind":"remote","encoding":E} for each encoding until known (30 s)',
    ]
)
def test_offer(cell: TvCell, screen: Screen, prysm: Prysm, record: Record) -> None:
    """The slim package, in the video state: with the lights off, the TV's offer is known for each encoding."""
    reached = in_state(screen, Shown.VIDEO, prysm, video_section(cell))
    prysm.set_target(Target.OFF)
    found: dict[str, prysm_api.ModesAnswer] = {}
    deadline = time.monotonic() + 30
    for encoding in Encoding:
        while True:
            answer = prysm_api.modes_answer(prysm.modes({'kind': 'remote', 'encoding': encoding}))
            if answer.get('status') == 'known' or time.monotonic() > deadline:
                break
            time.sleep(1)
        found[encoding] = answer
    evidence = {'state': reached, 'offers': found}
    unknown = [str(encoding) for encoding, answer in found.items() if answer.get('status') != 'known']
    if unknown:
        raise Failed(f'no offer known with lights off for {unknown}', evidence)
    if prysm.state()['target'] != Target.OFF:
        raise Failed('the receiver is not off', evidence)
    record('; '.join(f'{encoding}: {len(found[encoding]["modes"])} modes' for encoding in Encoding), evidence)


@only(Flavour.SLIM)
@planned(
    does=lambda found, cell, config: [
        *state_plan(found, Shown.VIDEO, config),
        f'for each encoding: PUT /api/v1/settings remote capture {CAPTURE[0]}x{CAPTURE[1]}@{CAPTURE[2]}, PUT '
        f'/api/v1/target screen_sync, GET /api/v1/state every second for {config.checks.seconds} s, '
        'GET /api/v1/preview/stream (the size that arrives)',
        *shown(found.sample_command(), found.state_command()),
        'PUT /api/v1/target off',
    ]
)
def test_encodings(cell: TvCell, config: Config, screen: Screen, prysm: Prysm, record: Record) -> None:
    """The slim package, in the video state: each encoding streams at 30 fps, and getState names it; the TV
    worker's CPU.

    Raw has no rate control: its frame rate is what the link to the receiver carries. So the evidence also
    holds, per encoding, the size of the frames that arrived and the TV's power and video state, to tell a slow
    link from a larger frame."""
    reached = in_state(screen, Shown.VIDEO, prysm, video_section(cell))
    results: dict[str, object] = {'state': reached}
    failed: list[str] = []
    summary: list[str] = []
    width, height, fps = CAPTURE
    for encoding in Encoding:
        try:
            ask_for(prysm, capture(Flavour.SLIM, width, height, fps, encoding=encoding))
        except Failed as error:
            results[encoding] = {'failed': str(error), 'evidence': error.evidence}
            failed.append(encoding)
            continue
        first = screen.tv.sample_worker()
        measured = rates(prysm.sample(config.checks.seconds))
        measured['tv_cpu'] = tv.cpu_percent(first, screen.tv.sample_worker())
        state = screen.tv.get_state() or tv.State()
        measured['tv_encoding'] = tv.encoding(state)
        measured['tv_power'], measured['tv_video'] = state.get('power'), state.get('video')
        measured['source'] = prysm.source_size()
        results[encoding] = measured
        summary.append(f'{encoding} {measured["fps_mean"]} fps ({measured["source"]})')
        if not near(measured['fps_mean'], fps, config.checks.fps_tolerance) or measured['tv_encoding'] != encoding:
            failed.append(encoding)
    prysm.set_target(Target.OFF)
    if failed:
        raise Failed(f'not {fps} fps, or another encoding, for {failed}', results)
    record(', '.join(summary), results)


@only(Flavour.SLIM)
@planned(
    does=lambda found, cell, config: [
        *state_plan(found, Shown.VIDEO, config),
        'PUT /api/v1/target off',
        *shown(found.state_command()),
        'PUT /api/v1/target screen_sync',
    ]
)
def test_pause_resume(cell: TvCell, screen: Screen, prysm: Prysm, record: Record) -> None:
    """The slim package, in the video state: the receiver's lights off pause the TV's capture; on again resumes
    it."""
    reached = in_state(screen, Shown.VIDEO, prysm, video_section(cell))
    prysm.set_target(Target.OFF)
    start = time.monotonic()
    paused = tv.wait_state(screen.tv.get_state, lambda s: tv.paused(s) and not tv.capturing(s), 15)
    paused_after = round(time.monotonic() - start, 1)
    prysm.set_target(Target.SCREEN_SYNC)
    start = time.monotonic()
    resumed = tv.wait_state(screen.tv.get_state, lambda s: tv.capturing(s) and not tv.paused(s), 20)
    evidence: dict[str, object] = {
        'state': reached,
        'paused': paused,
        'paused_after_s': paused_after,
        'resumed': resumed,
        'resumed_after_s': round(time.monotonic() - start, 1),
    }
    if paused is None or resumed is None:
        raise Failed('the TV did not pause and resume', evidence)
    record(f'paused after {paused_after} s, resumed after {evidence["resumed_after_s"]} s', evidence)


def _soak_plan(found: Tv, cell: TvCell, config: Config) -> list[str]:
    sizes = ', '.join(f'{w}x{h}@{f}' for w, h, f in SOAKS[cell.flavour])
    return [
        f'for {sizes}: the video state at that size, as below (the frames that arrive must be of that size), then '
        f'{config.tv.soak_seconds} s of GET /api/v1/state every second and, every 5 s:',
        *shown(found.sample_command()),
        *state_plan(found, Shown.VIDEO, config),
        f'PUT /api/v1/settings capture {CAPTURE[0]}x{CAPTURE[1]}@{CAPTURE[2]} again',
    ]


@planned(does=_soak_plan)
def test_soak(cell: TvCell, config: Config, screen: Screen, prysm: Prysm, record: Record) -> None:
    """The soak, in the video state: lighting for `[tv] soak_seconds` at each size (full: 640x360 and 1280x720;
    slim: 640x360, hardware H.264) with the frames arriving at that size and the frame rate within the
    tolerance; the TV worker's CPU and RSS from /proc."""
    results: list[tv.Soak] = []
    states: list[Reached] = []
    try:
        for width, height, fps in SOAKS[cell.flavour]:
            states.append(in_state(screen, Shown.VIDEO, prysm, capture(cell.flavour, width, height, fps)))
            result = tv.soak(screen.tv, prysm, config.tv.soak_seconds)
            result['size'] = f'{width}x{height}@{fps}'
            results.append(result)
            if not near(result['fps_mean'], fps, config.checks.fps_tolerance):
                raise Failed(
                    f'{result["fps_mean"]} fps over the soak at {result["size"]}, expected {fps}',
                    {'states': states, 'soaks': results},
                )
    finally:
        ask_for(prysm, video_section(cell))
    record('; '.join(f'{r["size"]}: {tv.soak_summary(r)}' for r in results), {'states': states, 'soaks': results})


def _ui_section(cell: TvCell, include_ui: bool = True) -> JsonObject:
    return capture(cell.flavour, *CAPTURE, include_ui=include_ui)


@planned(
    does=lambda found, cell, config: [
        *state_plan(found, Shown.UI, config, include_ui=True),
        f'GET /api/v1/state every second for {config.checks.seconds} s',
        'GET /api/v1/colors/stream for 3 s',
    ]
)
def test_ui_only(cell: TvCell, config: Config, screen: Screen, prysm: Prysm, record: Record) -> None:
    """The UI without video: the cast stopped, Prysm's app in front, the capture includes the UI.

    Video unavailable, the frames that arrive within the UI size (at most 320x180), at the rate within the
    tolerance, and the picture not black."""
    reached = in_state(screen, Shown.UI, prysm, _ui_section(cell))
    with stated(reached):
        measured = host_cells.frame_rate(prysm, config)
        detail, event = host_cells.not_black(prysm)
    record(
        f'{detail}; {host_cells.rate_detail(measured)}; {described(reached)}',
        {'state': reached, 'rates': measured, 'colours': event},
    )


def _quiet(prysm: Prysm, found: Tv, seconds: float) -> dict[str, object]:
    """Screen sync on and watched for `seconds`: no frames running (Prysm's session never running with frames,
    the preview without a frame) and nothing failing (no failure in Prysm's state or the TV's, the session not
    faulted); the samples as evidence, Failed when one did."""
    samples: list[dict[str, object]] = []
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        state = prysm.state()
        tv_state = found.get_state() or tv.State()
        samples.append(
            {
                'session': prysm_api.session(state),
                'running': prysm_api.running(state),
                'input': state.get('input'),
                'failure': prysm_api.failure(state),
                'tv_failure': tv_state.get('failure'),
                'tv_capturing': tv.capturing(tv_state),
            }
        )
        time.sleep(1)
    evidence: dict[str, object] = {'samples': samples, 'size': prysm.source_size()}
    framed = [s for s in samples if s['running']]
    failing = [s for s in samples if s['failure'] or s['tv_failure'] or s['session'] == SessionState.FAULTED]
    if framed or evidence['size']:
        raise Failed(f'frames arrive without video and without the UI ({evidence["size"]})', evidence)
    if failing:
        raise Failed('a failure without frames', evidence)
    return evidence


@planned(
    does=lambda found, cell, config: [
        'the UI state, as above, without frames to wait for:',
        *state_plan(found, Shown.UI, config, include_ui=False)[:4],
        f'PUT /api/v1/target screen_sync; for {QUIET_SECONDS} s every second GET /api/v1/state and:',
        *shown(found.state_command()),
        'GET /api/v1/preview/stream: no frame',
        'then the capture with the UI again: running with frames within 30 s, of the UI size',
    ]
)
def test_ui_off(cell: TvCell, screen: Screen, prysm: Prysm, record: Record) -> None:
    """No video and no UI: the same state, the capture without the UI: no frames, by design, and nothing fails
    (no failure in Prysm's state or the TV's, the session not faulted) for 10 s; with the UI on again the frames
    return, of the UI size."""
    screen.show(Shown.UI)
    reached = screen.confirm(Shown.UI)
    with stated(reached):
        _capture_saved(prysm, _ui_section(cell, include_ui=False))
        reply = prysm.set_target(Target.SCREEN_SYNC)
        if reply.status != 204:
            raise Failed(f'PUT /api/v1/target screen_sync: HTTP {reply.status}', reply.json())
        quiet = _quiet(prysm, screen.tv, QUIET_SECONDS)
    back = in_state(screen, Shown.UI, prysm, _ui_section(cell))
    record(
        f'no frames and no failure for {QUIET_SECONDS} s; frames again with the UI: {described(back)}',
        {'state': reached, 'quiet': quiet, 'again': back},
    )


@planned(
    does=lambda found, cell, config: [
        *state_plan(found, Shown.UI, config, include_ui=True),
        'a person puts the TV in standby with the remote, then wakes it',
        *shown(found.state_command()),
        f'(every 3 s: the TV not active, or not answering, within {STANDBY_SECONDS} s; after the wake, active, '
        'worker running and capturing within 90 s)',
        "then the UI state again (standby ended Prysm's app):",
        *state_plan(found, Shown.UI, config, include_ui=True),
    ]
)
def test_standby(cell: TvCell, screen: Screen, prysm: Prysm, record: Record) -> None:
    """Standby and wake (a person presses power), from the UI state: the TV leaves active (a TV in standby may
    not answer at all), then is active with the worker running and capturing within 90 s of waking, and the UI
    state is reached again (standby ended Prysm's app). The runner never puts the TV to sleep. Last: it ends
    the states the other checks set."""
    reached = in_state(screen, Shown.UI, prysm, _ui_section(cell))
    answer, note = ask_person(
        "Put the TV in standby with the remote's power button (do not unplug). Answer y once the screen is off.",
        (Answer.YES, Answer.SKIP),
    )
    if answer != Answer.YES:
        pytest.skip(f'standby skipped {note}'.strip())
    tell_person(f'waiting until the TV is not active, or does not answer (up to {STANDBY_SECONDS} s)')
    deadline = time.monotonic() + STANDBY_SECONDS
    while (asleep := screen.tv.get_state()) is not None and asleep['power'] == Power.ACTIVE:
        if time.monotonic() >= deadline:
            raise Failed(f'the TV is still active {STANDBY_SECONDS} s after the answer', asleep)
        time.sleep(3)
    evidence: dict[str, object] = {'state': reached, 'standby_state': asleep}
    answer, note = ask_person(
        'Wake the TV with the power button. Answer y when the picture is back.',
        (Answer.YES, Answer.SKIP),
    )
    if answer != Answer.YES:
        pytest.skip(f'wake skipped {note}'.strip())
    tell_person('waiting until the worker runs and captures again (up to 90 s)')
    awake = tv.wait_state(
        screen.tv.get_state,
        lambda s: s['power'] == Power.ACTIVE and tv.capturing(s),
        90,
        every=3,
    )
    evidence['wake_state'] = awake
    if awake is None:
        raise Failed('capture did not resume within 90 s of waking', evidence)
    # Standby ended Prysm's app: in front again, the UI state is as before, and the teardown has an app to close.
    tell_person("bringing Prysm's app to the front again and waiting for its frames")
    evidence['again'] = in_state(screen, Shown.UI, prysm, _ui_section(cell))
    record(f'standby seen: {asleep["power"] if asleep else "no answer"}; capture resumed', evidence)
