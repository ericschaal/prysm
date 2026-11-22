"""A TV as the shared TV checks see it (`Tv`, implemented by `webos.WebosTv`), the state it reports, and the
worker's load and soak computed from its readings."""

from collections.abc import Callable, Sequence
from enum import StrEnum
from itertools import pairwise
import time
from typing import Literal, NotRequired, Protocol, TypedDict, TypeVar

from prysm_acceptance import prysm_api
from prysm_acceptance.commands import Command

CLOCK_TICKS = 100  # USER_HZ on the TV's ARM kernel (assumed; see the report)


class Encoding(StrEnum):
    """How the slim package streams the picture."""

    H264_HARDWARE = 'h264_hardware'
    H264_SOFTWARE = 'h264_software'
    RAW = 'raw'


class Power(StrEnum):
    """The TV's power as its state reports it."""

    UNKNOWN = 'unknown'
    ACTIVE = 'active'
    SCREEN_SAVER = 'screen_saver'
    ASLEEP = 'asleep'


class Video(StrEnum):
    """Video on the TV's main output as its state reports it: available only while the power is active."""

    UNKNOWN = 'unknown'
    UNAVAILABLE = 'unavailable'
    AVAILABLE = 'available'


class StoppedWorker(TypedDict):
    phase: Literal['stopped']


class RunningWorker(TypedDict):
    """A running worker and its report. Both packages: frames flow (`capturing`) at `fps`. Full only: `portal`.
    Slim only: `encoding` and `paused` (the receiver paused the stream)."""

    phase: Literal['running']
    capturing: bool
    fps: NotRequired[int]
    portal: NotRequired[str]
    encoding: NotRequired[str]
    paused: NotRequired[bool]


class StoppingWorker(TypedDict):
    """A worker being stopped; the full one's portal may still answer."""

    phase: Literal['stopping']
    portal: NotRequired[str]


class RetryingWorker(TypedDict):
    phase: Literal['retrying']
    seconds: int


Worker = StoppedWorker | RunningWorker | StoppingWorker | RetryingWorker


class TvFailure(TypedDict, total=False):
    kind: str
    message: str


class State(TypedDict, total=False):
    """The TV's state as the shared checks read it: its power and video, and Prysm's worker on it (its report is
    read through `running`, `capturing`, `fps`, `paused`, `encoding` and `portal`)."""

    power: Power
    video: Video
    worker: Worker
    failure: TvFailure
    log: list[str]


def running(state: State) -> RunningWorker | None:
    """The worker and its report while it runs; `None` otherwise."""
    worker = state.get('worker')
    if worker is None or worker['phase'] != 'running':
        return None
    return worker


def capturing(state: State) -> bool:
    """Frames flow from a running worker."""
    report = running(state)
    return report is not None and report['capturing']


def fps(state: State) -> int | None:
    """The running worker's frame rate; `None` while none is measured."""
    report = running(state)
    return None if report is None else report.get('fps')


def paused(state: State) -> bool:
    """Slim: the receiver paused the running worker's stream (its lighting is off)."""
    report = running(state)
    return report is not None and report.get('paused', False)


def encoding(state: State) -> str | None:
    """Slim: the encoding the running worker sends in."""
    report = running(state)
    return None if report is None else report.get('encoding')


def portal(state: State) -> str | None:
    """Full: the portal's LAN address while the worker runs or stops."""
    worker = state.get('worker')
    if worker is None:
        return None
    if worker['phase'] == 'running' or worker['phase'] == 'stopping':
        return worker.get('portal')
    return None


class Reading(TypedDict):
    """`Tv.sample_worker`: one reading of the TV's uptime and load and the worker's CPU ticks and RSS."""

    uptime: float
    load: str
    ticks: int | None
    rss_kb: int | None


class Load(TypedDict):
    cpu_mean: float | None
    cpu_max: float | None
    rss_max_kb: int | None
    load: list[str]


class Soak(prysm_api.Rates, Load):
    """`soak`: the rates Prysm received and the TV worker's load over `seconds`."""

    seconds: float
    size: NotRequired[str]


class Tv(Protocol):
    """A TV with Prysm installed, as the shared checks drive and read it. The `*_command`s are what the dry run
    shows for `get_state`, `foreground`, `launch_app` and `sample_worker`."""

    @property
    def prysm_app(self) -> str:
        """Prysm's own app: in front in the UI state."""
        ...

    @property
    def cast_app(self) -> str:
        """The TV's Cast receiver: in front while the clip is cast."""
        ...

    def get_state(self) -> State | None:
        """The TV's state, or `None` when Prysm's service does not answer."""
        ...

    def foreground(self) -> str:
        """The app in front; empty when the TV names none."""
        ...

    def launch_app(self) -> None:
        """Prysm's own app in front."""
        ...

    def close_app(self) -> None:
        """Prysm's own app closed."""
        ...

    def sample_worker(self) -> Reading:
        """One reading of the TV's uptime and load and the worker's CPU ticks and RSS (None without a worker)."""
        ...

    def state_command(self) -> Command: ...

    def foreground_command(self) -> Command: ...

    def launch_command(self) -> Command: ...

    def sample_command(self) -> Command: ...


Read = TypeVar('Read', bound=State)


def wait_state(
    get_state: Callable[[], Read | None], check: Callable[[Read], object], seconds: float, every: float = 2.0
) -> Read | None:
    """`get_state()` once `check` holds, polled every `every` s; None when `seconds` pass first."""
    deadline = time.monotonic() + seconds
    while True:
        state = get_state()
        if state is not None and check(state):
            return state
        if time.monotonic() >= deadline:
            return None
        time.sleep(every)


def cpu_percent(first: Reading, second: Reading) -> float | None:
    """The worker's CPU between two readings, in percent of one core."""
    if first['ticks'] is None or second['ticks'] is None or second['uptime'] <= first['uptime']:
        return None
    return round(100 * (second['ticks'] - first['ticks']) / CLOCK_TICKS / (second['uptime'] - first['uptime']), 1)


def load_summary(readings: Sequence[Reading]) -> Load:
    cpu = [c for c in (cpu_percent(a, b) for a, b in pairwise(readings)) if c is not None]
    rss = [r['rss_kb'] for r in readings if r['rss_kb']]
    return {
        'cpu_mean': round(sum(cpu) / len(cpu), 1) if cpu else None,
        'cpu_max': max(cpu, default=None),
        'rss_max_kb': max(rss, default=None),
        'load': [r['load'] for r in readings][-1:],
    }


def soak(found: Tv, prysm: prysm_api.Prysm, seconds: float) -> Soak:
    """`prysm`'s input every second and the TV worker's load every 5 s, for `seconds`."""
    inputs: list[prysm_api.Input | None] = []
    readings: list[Reading] = []
    deadline = time.monotonic() + seconds
    next_reading = 0.0
    while time.monotonic() < deadline:
        if time.monotonic() >= next_reading:
            readings.append(found.sample_worker())
            next_reading = time.monotonic() + 5
        inputs.append(prysm.state().get('input'))
        time.sleep(1)
    readings.append(found.sample_worker())
    return {**prysm_api.rates(inputs), **load_summary(readings), 'seconds': seconds}


def soak_summary(result: Soak) -> str:
    return (
        f'{result["fps_mean"]} fps (min {result["fps_min"]}), worker CPU {result["cpu_mean"]} % '
        f'(max {result["cpu_max"]}), RSS {result["rss_max_kb"]} kB'
    )
