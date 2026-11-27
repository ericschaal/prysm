"""Prysm's portal API (crates/surfaces/web-portal/HTTP-CONTRACT.md), as the checks use it.

The replies are typed here, at the edge, as the TypedDicts below: the parts of
the contract the checks read. They are not validated; a reply of another
shape fails the check that reads it, as before.
"""

from collections.abc import Callable, Sequence
import copy
import json
import re
import statistics
import time
from typing import Literal, NotRequired, TypeAlias, TypedDict, cast, overload

from prysm_acceptance import web
from prysm_acceptance.commands import Command
from prysm_acceptance.transport import Transport
from prysm_acceptance.web import JsonObject
from prysm_acceptance.wire_generated import CaptureMode, LightingControl, Picture, SessionState, Span, Target

# Settings stay opaque JSON: the checks read and change a few fields (`section`)
# and send the rest back as they found it, so Prysm may add or change any other.
Settings: TypeAlias = JsonObject

# The settings file's `format` (crates/adapters/settings-file/README.md): a file
# Prysm reads starts with it; the portal's settings never carry it.
SETTINGS_FORMAT = 1


class ModesAnswer(TypedDict):
    """`POST /api/v1/capture/modes`."""

    status: str
    modes: NotRequired[list[CaptureMode]]


class Size(TypedDict):
    width: int
    height: int
    fps: int


class Input(TypedDict, total=False):
    fps: float
    picture: Picture
    sent_fps: float
    behind: str


class Failure(TypedDict, total=False):
    kind: str
    message: str
    field: str
    section: str


class Session(TypedDict):
    """`State.session`: `failure` only while stopped or showing the lights' effects, `message` only while
    faulted."""

    state: SessionState
    failure: NotRequired[Failure]
    message: NotRequired[str]


class Device(TypedDict, total=False):
    """`State.device`: what the lights report about themselves."""

    issue: str
    brightness_percent: int
    led_count: int


class State(TypedDict, total=False):
    """`GET /api/v1/state`."""

    version: str
    target: Target
    settings: Settings
    revision: int
    session: Session
    input: Input | None
    device: Device
    control: LightingControl


class Candidate(TypedDict):
    label: str
    fields: dict[str, str]


Edge: TypeAlias = Literal['top', 'right', 'bottom', 'left']
EDGES: tuple[Edge, ...] = ('top', 'right', 'bottom', 'left')


class ColorsEvent(TypedDict, total=False):
    """One event of `GET /api/v1/colors/stream`: `colors` with each edge's entries as [r, g, b]."""

    type: str
    top: list[list[int]]
    right: list[list[int]]
    bottom: list[list[int]]
    left: list[list[int]]


class Rates(TypedDict):
    """`rates`: input and sent frame rates over a sample, and what the callers add to it."""

    samples: int
    fps_mean: float
    fps_min: float
    sent_mean: float
    behind: list[str]
    expected: NotRequired[int]
    tv_cpu: NotRequired[float | None]
    tv_encoding: NotRequired[str | None]
    tv_power: NotRequired[str | None]
    tv_video: NotRequired[str | None]
    source: NotRequired[str | None]


class Prysm:
    def __init__(self, base: str, timeout: float = 20.0) -> None:
        self.base = base.rstrip('/')
        self.timeout = timeout

    def _url(self, path: str) -> str:
        return self.base + path

    def state(self) -> State:
        return cast('State', web.get_json(self._url('/api/v1/state')))

    def set_target(self, target: Target) -> web.Reply:
        """`PUT /api/v1/target`; the reply (204 once applied)."""
        if target not in tuple(Target):
            raise ValueError(target)
        return web.request('PUT', self._url('/api/v1/target'), {'target': target}, timeout=self.timeout)

    def replace_settings(self, settings: Settings) -> web.Reply:
        """Save whole settings against the current revision; the reply."""
        revision = self.state()['revision']
        return web.request(
            'PUT', self._url('/api/v1/settings'), {'expected': revision, 'settings': settings}, timeout=self.timeout
        )

    def change_settings(self, change: Callable[[Settings], object]) -> web.Reply:
        """Apply `change(settings)` to a copy of the current settings and save it."""
        state = self.state()
        settings = copy.deepcopy(state['settings'])
        change(settings)
        return web.request(
            'PUT',
            self._url('/api/v1/settings'),
            {'expected': state['revision'], 'settings': settings},
            timeout=self.timeout,
        )

    def version(self) -> str | None:
        """The running version, `prysm_build_info{version}` in `GET /metrics` (served with `--metrics`)."""
        text = web.request('GET', self._url('/metrics'), timeout=self.timeout).body.decode(errors='replace')
        found = re.search(r'^prysm_build_info\{version="([^"]*)"\} 1$', text, re.MULTILINE)
        return found[1] if found else None

    def modes(self, capture: dict[str, object]) -> web.Reply:
        return web.request('POST', self._url('/api/v1/capture/modes'), capture, timeout=self.timeout)

    def candidates(self, section: str, kind: str) -> list[Candidate]:
        return cast(
            'list[Candidate]', web.get_json(self._url(f'/api/v1/candidates/{section}/{kind}'), timeout=self.timeout)
        )

    def calibration(self, pattern: str | None) -> web.Reply:
        if pattern is None:
            return web.request('DELETE', self._url('/api/v1/calibration'), timeout=self.timeout)
        return web.request(
            'PUT', self._url('/api/v1/calibration'), {'pattern': pattern, 'uncorrected': False}, timeout=self.timeout
        )

    @overload
    def colors(self, seconds: float = 3.0, timed: Literal[False] = False) -> list[ColorsEvent]: ...
    @overload
    def colors(self, seconds: float, timed: Literal[True]) -> list[tuple[float, ColorsEvent]]: ...
    @overload
    def colors(self, seconds: float, timed: bool) -> list[ColorsEvent] | list[tuple[float, ColorsEvent]]: ...
    def colors(self, seconds: float = 3.0, timed: bool = False) -> list[ColorsEvent] | list[tuple[float, ColorsEvent]]:
        """The `ColorsEvent`s the colours stream sends within `seconds`.

        `timed`: each as (seconds since the request, event), by its arrival here. The stream
        sends only when the colours change, so an event holds until the next."""
        url = self._url('/api/v1/colors/stream')
        if timed:
            return cast('list[tuple[float, ColorsEvent]]', web.events(url, seconds, timed=True))
        return cast('list[ColorsEvent]', web.events(url, seconds))

    def source_size(self, seconds: float = 3.0) -> str | None:
        """The size of the frames that arrive, WIDTHxHEIGHT, from the preview stream's first frame within
        `seconds`; None without one. A source may deliver another size than the settings ask for."""
        for event in web.events(self._url('/api/v1/preview/stream'), seconds, limit=3):
            if isinstance(event, dict) and event.get('type') == 'frame':
                return f'{event.get("source_width")}x{event.get("source_height")}'
        return None

    def wait(self, check: Callable[[State], object], seconds: float, every: float = 0.5) -> State | None:
        """The first state for which `check(state)` holds; `None` after `seconds`."""
        deadline = time.monotonic() + seconds
        while True:
            state = self.state()
            if check(state):
                return state
            if time.monotonic() >= deadline:
                return None
            time.sleep(every)

    def sample(self, seconds: float, every: float = 1.0) -> list[Input | None]:
        """`input` every `every` s for `seconds` (absent while no session runs)."""
        found: list[Input | None] = []
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            found.append(self.state().get('input'))
            time.sleep(every)
        return found


def timed_lines(text: str) -> list[tuple[float, ColorsEvent]]:
    """`commands.COLOURS_READER`'s output, `<seconds> <payload>` per line, as `Prysm.colors(timed=True)` gives
    events; a line of another shape raises ValueError."""
    found: list[tuple[float, ColorsEvent]] = []
    for line in text.splitlines():
        at, _, payload = line.partition(' ')
        found.append((float(at), cast('ColorsEvent', json.loads(payload))))
    return found


def colours_on_device(run: Transport, command: Command) -> list[tuple[float, ColorsEvent]]:
    """The timed colours `command` (`commands.device_colours`) reads on the device Prysm runs on; a non-zero exit
    raises `CommandFailed`."""
    return timed_lines(run.run(command).stdout)


def section(settings: Settings, name: str) -> JsonObject:
    """One section of the settings (`output`, `capture`, ...), to read or change in place."""
    return cast('JsonObject', settings[name])


def modes_answer(reply: web.Reply) -> ModesAnswer:
    """The reply of `Prysm.modes`."""
    return cast('ModesAnswer', reply.json())


def session(state: State | None) -> SessionState | None:
    return (state or State()).get('session', cast('Session', {})).get('state')


def failure(state: State | None) -> Failure | None:
    """The failure the session carries, if any."""
    return (state or State()).get('session', cast('Session', {})).get('failure')


def running(state: State) -> bool:
    arriving = (state.get('input') or Input()).get('picture') == Picture.ARRIVING
    return session(state) == SessionState.SCREEN_SYNC and arriving


def rates(samples: Sequence[Input | None]) -> Rates:
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


def near(value: float, expected: float, tolerance: float) -> bool:
    return abs(value - expected) <= expected * tolerance


def describe_modes(modes: Sequence[CaptureMode]) -> list[str]:
    """One line per mode, `640x360 @ 30` or spans `176-1920/2 x ... @ 1-30`."""

    def span(s: Span) -> str:
        return str(s['min']) if s['min'] == s['max'] else f'{s["min"]}-{s["max"]}/{s["step"]}'

    return [f'{span(m["width"])} x {span(m["height"])} @ {span(m["fps"])}' for m in modes]


def offers(modes: Sequence[CaptureMode], size: Size) -> bool:
    """Whether one of `modes` takes exactly this size and rate (the check asks; it resolves nothing)."""

    def within(span: Span, value: int) -> bool:
        return span['min'] <= value <= span['max'] and (value - span['min']) % max(span['step'], 1) == 0

    return any(
        within(m['width'], size['width']) and within(m['height'], size['height']) and within(m['fps'], size['fps'])
        for m in modes
    )
