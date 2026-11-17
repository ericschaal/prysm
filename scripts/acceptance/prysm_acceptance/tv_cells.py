"""The TV cells: a TV package installed on the owner's LG TV, lighting WLED itself (full) or streaming to a
receiver that does (slim).

`acceptance tv` and `acceptance all --tv-is-free` run this module with pytest
and the `bench` plugin; a cell is the `cell` parameter (`tv-full`,
`tv-slim-mac`, `tv-slim-linux`). Its fixtures set up, in order: the package
(and the slim receiver's binary), the TV (reached over SSH, woken with
wake-on-LAN when `[tv] mac` is set, and what of Prysm is on it: refused, or
replaced after the WLED snapshot; nobody using it: only the home screen,
Prysm or the Cast receiver in front), the slim receiver's target (the host
cells' seam), WLED's snapshot, a Prysm already on the TV replaced, the
package installed, the Prysm that lights WLED (the TV's own portal, or the
receiver with the TV streaming to it), what WLED should see, the test clip,
and the `screen`: what the TV shows.

Every check that measures runs in a state of the TV it declares (`Shown`):
the runner sets it, confirms it from the TV itself, and only then measures
(`in_state`). VIDEO: the clip cast to the TV's Cast receiver with a known
overlay over its bottom, video available, the frames of the size asked.
UI: the cast stopped, Prysm's own app in front, video unavailable, the frames
of the TV's UI size. A state not reached fails the check, saying which part.
The checks follow in this module's order, the video ones first, so the clip
is cast once and stopped once. The teardowns undo it in reverse, also after
a failure or Ctrl-C: the cast stopped and the clip server closed, Prysm's app
closed, the receiver stopped and removed, the package uninstalled with a
check that nothing of Prysm is left, WLED restored. Each fixture and check
says what it does in its docstring and, for the dry run, in `planned`.

Every TV command is a `tv.*` template in `commands.py`; the runner touches
only Prysm's own files there (`commands.TV_READ_PATHS`, `TV_WRITE_PATHS`),
and launches or closes no app but Prysm's.
"""

from collections.abc import Generator, Iterator
import contextlib
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
import socket
import time
from typing import TypedDict, cast
from urllib.parse import urlsplit
import warnings

import pytest

from prysm_acceptance import artifact as artifacts
from prysm_acceptance import (
    commands,
    edges_clip,
    feed,
    feed_cast,
    host,
    host_cells,
    installed,
    options,
    prysm_api,
    transport,
    tv,
)
from prysm_acceptance.artifact import Artifact
from prysm_acceptance.bench import Planner, only, unavailable, uninterrupted, warn_at_end
from prysm_acceptance.checks import Answer, Bench, Failed, Skipped, ask_person
from prysm_acceptance.config import SSH, Config, Installed
from prysm_acceptance.host import Host, Sender, shown
from prysm_acceptance.host_cells import Clip, Record, processed, wled_as_found, wled_restored, wled_saved
from prysm_acceptance.options import Flavour, TvCell
from prysm_acceptance.prysm_api import Prysm, SessionState, Target, near, rates
from prysm_acceptance.tv import Encoding, LunaReply, Power, Tv, Video
from prysm_acceptance.web import JsonObject
from prysm_acceptance.wled_api import Wled

CELLS = list(options.TV_CELLS)
planned = Planner[TvCell]()
# The capture every cell starts with, and the soaks (width, height, fps): the full package captures at each
# size, the slim package streams hardware H.264.
CAPTURE = (640, 360, 30)
SOAKS = {Flavour.FULL: [(640, 360, 30), (1280, 720, 30)], Flavour.SLIM: [(640, 360, 30)]}
# The TV's UI capture is at most this size, whatever was asked (adapters/capture-webos settings.rs `ui_size`).
UI_LIMIT = (320, 180)
# The apps in front while the TV is free: none named, the home screen (also under the screen saver), Prysm's
# own app, and the Cast receiver the clip plays in. Anything else in front: someone is using the TV.
HOME_APP = 'com.webos.app.home'
CAST_APP = 'com.webos.chromecast'
FREE_APPS = ('', HOME_APP, commands.APP, CAST_APP)
# How long the TV may take to report a state the runner set.
CONFIRM_SECONDS = 20
# How long the check without frames watches that none come and nothing fails.
QUIET_SECONDS = 10
# For the dry run, where the configuration names no TV.
EXAMPLE_TV = 'root@TV'
# Set once a TV cell of this run starts installing its package: from then on the data files on the TV
# (which no uninstall removes) are the run's own, not the owner's.
INSTALLED_BY_RUN = pytest.StashKey[bool]()


@dataclass(frozen=True)
class OnTv:
    """The TV, and what of Prysm was on it before the run (`Tv.found`)."""

    tv: Tv
    found: list[str]


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
        state = self.tv.get_state() or LunaReply()
        (self.out_dir / 'tv-worker.log').write_text(''.join(line + '\n' for line in state.get('log', [])))
        if self.receiver is not None:
            self.receiver.fetch()


class Shown(StrEnum):
    """What the TV shows while a check measures: the state the runner sets and confirms."""

    VIDEO = 'video'  # the clip cast and playing with its overlay: the Cast receiver in front, video available
    UI = 'ui'  # the cast stopped, Prysm's own app in front: video unavailable, the TV's UI captured


# Per state: the video getState must report and the app in front.
WANTED: dict[Shown, tuple[Video, str]] = {
    Shown.VIDEO: (Video.AVAILABLE, CAST_APP),
    Shown.UI: (Video.UNAVAILABLE, commands.APP),
}


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
        """`wanted` as getState and the application manager report it within `CONFIRM_SECONDS`: the power active,
        the video exactly available or unavailable (unknown is not reached), the state's app in front; for
        VIDEO, once the player's own overlay has cleared, the clip still PLAYING. Else Failed, naming what was
        not reached."""
        video, app = WANTED[wanted]
        deadline = time.monotonic() + CONFIRM_SECONDS
        while True:
            state = self.tv.get_state() or LunaReply()
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
        state = self.tv.get_state() or LunaReply()
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


def _ssh(config: Config) -> str:
    return config.tv.ssh or EXAMPLE_TV


def _ipk(cell: TvCell, config: Config) -> Path | None:
    return config.tv.ipk(cell.flavour)


def _settings_document(config: Config, capture: JsonObject) -> prysm_api.Settings:
    return host.settings(host.wled_output(config), capture)


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
    processed(prysm, seconds=30)


def _receiver_seam(cell: TvCell, config: Config, run: transport.Transport, out_dir: Path) -> Host:
    """The slim receiver: the host cells' binary seam on `[mac]` or `[linux]`, listening on the LAN (the TV must
    reach it) on the platform's listen port."""
    machine = config.machine(cast('options.Platform', cell.receiver))
    binary = machine.artifact() or Path('BINARY')
    if machine.target.startswith(SSH):
        return host.SshBinary(run, machine.target.removeprefix(SSH), binary, machine.listen, out_dir, config.wled)
    port = machine.listen.rsplit(':', 1)[1]
    return host.LocalBinary(run, binary, f'0.0.0.0:{port}', out_dir, config.wled)


def _instance(seam: Host, tv_host: str) -> str:
    """The receiver as the TV reaches it: this computer's address towards the TV, or the Linux target's."""
    url = urlsplit(seam.url)
    if isinstance(seam, host.LocalBinary):
        address = host.local_address(tv_host)
    else:
        address = socket.gethostbyname(url.hostname or '')
    return f'http://{address}:{url.port}'


def _tv_host(config: Config) -> str:
    return config.tv.host or EXAMPLE_TV.rsplit('@', 1)[-1]


def owners(found: list[str], installed_by_run: bool) -> list[str]:
    """What of Prysm on the TV is the owner's: all of `found`, but the data files once a cell of this run has
    installed a package (they are then this run's own)."""
    if not installed_by_run:
        return found
    return [item for item in found if not item.startswith(commands.DATA_DIR + '/')]


# ---- fixtures, in the order they set up ------------------------------------


def _artifact_plan(cell: TvCell, config: Config) -> list[str]:
    ipk = _ipk(cell, config)
    receiver = config.machine(cell.receiver).binary if cell.receiver else ''
    return [
        f'[tv] ipk_{cell.flavour}: {config.tv.ipk_full if cell.flavour == Flavour.FULL else config.tv.ipk_slim}; '
        'its SHA-256 and age, and the version and commit from manifest.json next to it when the hash matches',
        f'$ {commands.verify_ipk(str(ipk or "IPK")).display()}   (by content: nothing outside the app and service)',
        *([f'[{cell.receiver}] binary (the receiver): {receiver or "(not set)"}'] if cell.receiver else []),
        'the clip cast to the TV: FFmpeg on PATH for the clip, PyChromecast',
    ]


@pytest.fixture(scope='module')
@planned(does=_artifact_plan)
def artifact(cell: TvCell, config: Config, bench: Bench, request: pytest.FixtureRequest) -> Artifact:
    """The package under test, checked by content, the slim receiver's binary, and the feed's tools.

    A missing package, receiver binary or tool, or no TV configured, skips the cell, or fails it when it was
    named."""
    if not config.wled:
        pytest.fail('wled.address is not set (config file or --wled)', pytrace=False)
    if sum(config.leds.as_settings().values()) == 0:
        pytest.fail('leds: no LEDs configured', pytrace=False)
    if not config.tv.target:
        unavailable(request, cell, 'no TV: [tv] target is not set')
    ipk = _ipk(cell, config)
    if ipk is None or not ipk.is_file():
        unavailable(request, cell, f'no package: [tv] ipk_{cell.flavour} is {ipk or "not set"}')
    if cell.receiver:
        binary = config.machine(cell.receiver).artifact()
        if binary is None or not binary.is_file():
            unavailable(request, cell, f'no receiver: [{cell.receiver}] binary is {binary or "not set"}')
    host_cells.tools(request, cell, feed.on_tv(config.tv), clip=True)
    verified = bench.run(commands.verify_ipk(str(ipk)), check=False)
    if not verified.ok:
        pytest.fail(f'{ipk.name} is refused by ipk.py verify: {(verified.stderr or verified.stdout)[-500:]}')
    found = artifacts.inspect(ipk)
    facts = found.facts()
    warning = artifacts.older_than_checkout(found, bench.transport)
    if warning:
        facts['warning'] = warning
        warnings.warn(warning, stacklevel=1)
    bench.save('artifact.json', facts)
    bench.log('Package: ' + ', '.join(f'{key} {value}' for key, value in facts.items()))
    return found


def _target_plan(cell: TvCell, config: Config) -> list[str]:
    ssh = _ssh(config)
    return [
        *shown(commands.tv_reachable(ssh)),
        *_wake_plan(config),
        *shown(commands.tv_foreground(ssh)),
        f'(saved as tv-foreground.json; another app than {", ".join(app or "none" for app in FREE_APPS)} in '
        'front: someone uses the TV, the cell is skipped, or fails when named, before anything changes)',
        *shown(commands.tv_processes(ssh)),
        '(the old io.prysm.capture app running fails the cell)',
        *shown(
            commands.tv_paths(ssh),
            commands.tv_processes(ssh),
            commands.tv_get_state(ssh),
            *(commands.tv_read_data(ssh, name) for name in commands.DATA_FILES),
        ),
        '(anything of Prysm found, its data files included: '
        + (
            'refused, nothing is changed; [tv] installed = "refuse")'
            if config.tv.installed == Installed.REFUSE
            else 'replaced after the WLED snapshot; [tv] installed = "replace")'
        ),
    ]


def _wake_plan(config: Config) -> list[str]:
    if not config.tv.mac:
        return ['(no [tv] mac: a TV that does not answer skips the cell, or fails it when named)']
    return [
        f'only when it does not answer: $ {commands.wake_on_lan(config.tv.mac, "192.0.2.10").display()}   '
        "(192.0.2.10: this computer's address towards the TV)",
        f'then {commands.tv_reachable(_ssh(config)).display()} every 5 s for up to {tv.WAKE_SECONDS} s',
    ]


@pytest.fixture(scope='module')
@planned(does=_target_plan)
def target(cell: TvCell, config: Config, bench: Bench, request: pytest.FixtureRequest, artifact: Artifact) -> OnTv:
    """The TV: reachable over SSH (woken first when `[tv] mac` is set), free, and what of Prysm is on it.

    Free: the app in front (saved, and never brought back at the end) is none, the home screen, Prysm's or the
    Cast receiver; another means someone uses the TV. With `[tv] installed = "refuse"` anything of Prysm found
    (the app, the service, its manifests or boot hook, a process, the Luna service, the data files an uninstall
    leaves) skips the cell, or fails it when named, before anything changes."""
    found = Tv(bench.transport, config.tv.ssh)
    unreachable = found.reach(config.tv.mac, lambda: host.local_address(_tv_host(config)))
    if unreachable:
        unavailable(request, cell, unreachable)
    front = found.foreground()
    bench.save('tv-foreground.json', {'foreground': front})
    if front not in FREE_APPS:
        unavailable(request, cell, f'someone is using the TV: {front} is in front')
    old = [line for line in found.processes() if 'io.prysm.capture' in line[-1]]
    if old:
        pytest.fail(f'the old io.prysm.capture app is running; remove it first (docs/users/webos.md): {old}')
    prysm = owners(found.found(), request.config.stash.get(INSTALLED_BY_RUN, False))
    bench.save('tv-found.json', prysm)
    if config.tv.installed == Installed.REFUSE:
        reason = installed.refusal(found, prysm)
        if reason:
            unavailable(request, cell, reason)
    return OnTv(found, prysm)


def _receiver_plan(cell: TvCell, config: Config) -> list[str]:
    if not cell.receiver:
        return []
    return _receiver_seam(cell, config, transport.Refusing(), Path('RUN_DIR/CELL')).plan(None, '').preflight


@pytest.fixture(scope='module')
@planned(does=_receiver_plan)
def receiver(cell: TvCell, config: Config, bench: Bench, request: pytest.FixtureRequest, target: OnTv) -> Host | None:
    """Slim: where the receiver runs (the host cells' seam on `[mac]` or `[linux]`), and whether it can.

    Over SSH: reachable without a prompt, Linux on the binary's machine, the listen port free."""
    if not cell.receiver:
        return None
    seam = _receiver_seam(cell, config, bench.transport, bench.out_dir)
    try:
        reason = seam.preflight(None, '')
    except transport.CommandFailed as error:
        reason = f'the receiver preflight failed: {error}'
    if reason:
        unavailable(request, cell, reason)
    return seam


@pytest.fixture(scope='module')
@planned(does=wled_saved, undoes=wled_restored)
def wled(config: Config, bench: Bench, receiver: Host | None) -> Iterator[Wled]:
    """WLED as found, saved before anything changes and put back last, also after a failure or Ctrl-C."""
    with wled_as_found(config, bench) as client:
        yield client


def _cleared_plan(cell: TvCell, config: Config) -> list[str]:
    ssh = _ssh(config)
    return host_cells.replaced(
        'tv',
        config.tv.installed,
        [
            *shown(*(commands.tv_read_data(ssh, name) for name in commands.DATA_FILES), commands.tv_get_state(ssh)),
            '(nothing answering while Prysm is installed stops the cell, nothing removed)',
            'the full package:',
            *shown(commands.tv_get_state(ssh)),
            'GET PORTAL/api/v1/state (PORTAL as getState reports it, within 40 s)',
            'then, unless only data files were found:',
            *_uninstall_plan(config),
        ],
    )


@pytest.fixture(scope='module')
@planned(does=_cleared_plan)
def cleared(bench: Bench, request: pytest.FixtureRequest, target: OnTv, wled: Wled) -> None:
    """With `[tv] installed = "replace"`, the Prysm the TV had backed up and uninstalled.

    After the WLED snapshot, so Prysm lighting WLED is refused before anything is removed. Its data files
    stay on the TV (the runner never deletes them); the run changes them, and the backup holds them as they
    were."""
    if not target.found:
        return
    path = bench.out_dir / installed.BACKUP
    bench.log(installed.replace(target.tv, target.found, path, lambda text: warn_at_end(request, text)))


def _install_plan(ssh: str, ipk: str) -> list[str]:
    return shown(
        commands.verify_ipk(ipk),
        commands.tv_stage(ssh, ipk),
        commands.tv_install(ssh),
        commands.tv_unstage(ssh),
        commands.tv_setup(ssh),
        commands.tv_get_state(ssh),
    )


def _uninstall_plan(config: Config) -> list[str]:
    ssh = _ssh(config)
    return [
        *shown(
            commands.tv_supervisor_uninstall(ssh),
            commands.tv_remove_app(ssh),
            commands.tv_paths(ssh),
            commands.tv_processes(ssh),
            commands.tv_get_state(ssh),
        ),
        f'(anything of {", ".join(commands.INSTALLED_PATHS)}, {commands.STAGING}, a process or the Luna service '
        f'still there fails; {commands.DATA_DIR} stays, as documented)',
    ]


@pytest.fixture(scope='module')
@planned(
    does=lambda cell, config: _install_plan(_ssh(config), str(_ipk(cell, config) or 'IPK')),
    undoes=lambda cell, config: [
        'whether anything of the package is there:',
        *shown(
            commands.tv_paths(_ssh(config)), commands.tv_processes(_ssh(config)), commands.tv_get_state(_ssh(config))
        ),
        'if so, it is uninstalled, then nothing of Prysm may be left:',
        *_uninstall_plan(config),
    ],
)
def package(
    cell: TvCell, config: Config, request: pytest.FixtureRequest, target: OnTv, cleared: None
) -> Iterator[LunaReply]:
    """The package installed the documented way (verified, staged, appInstallService, unstaged, setup.sh).

    At the end, also after a failure, it is uninstalled (the supervisor's uninstall, then the app removed
    through the TV's install service) and nothing of Prysm may be left but its data files."""
    request.config.stash[INSTALLED_BY_RUN] = True
    try:
        yield target.tv.install(str(_ipk(cell, config)))
    finally:
        with uninterrupted():
            left = target.tv.uninstall() if target.tv.installed() else []
        if left:
            pytest.fail(f'the uninstall left on the TV: {", ".join(left)}')


def _prysm_plan(cell: TvCell, config: Config) -> list[str]:
    width, height, fps = CAPTURE
    if cell.flavour == Flavour.FULL:
        return [
            *shown(commands.tv_get_state(_ssh(config))),
            'GET PORTAL/api/v1/state until it answers (PORTAL as getState reports it, within 40 s)',
            f'PUT PORTAL/api/v1/settings: WLED output, neutral processing, the webos capture {width}x{height}@{fps} '
            'without the UI',
        ]
    seam = _receiver_seam(cell, config, transport.Refusing(), Path('RUN_DIR/CELL')).plan(None, '')
    ssh = _ssh(config)
    return [
        f'write RUN_DIR/CELL/headless-settings.json: WLED output, neutral processing, the remote capture '
        f'{width}x{height}@{fps} hardware H.264 without the UI',
        *seam.start,
        *shown(
            commands.tv_set_preferences(ssh, False, 'http://192.0.2.20:18787'),
            commands.tv_set_enabled(ssh, True),
        ),
        '(instance: the receiver as the TV reaches it, shown as http://192.0.2.20:18787)',
    ]


def _prysm_undo(cell: TvCell, config: Config) -> list[str]:
    if cell.flavour == Flavour.FULL:
        return []
    return _receiver_seam(cell, config, transport.Refusing(), Path('RUN_DIR/CELL')).plan(None, '').stop


@pytest.fixture(scope='module')
@planned(does=_prysm_plan, undoes=_prysm_undo)
def prysm(
    cell: TvCell, config: Config, bench: Bench, target: OnTv, receiver: Host | None, package: LunaReply
) -> Iterator[Prysm]:
    """The Prysm that lights WLED, with this run's settings (WLED output, neutral processing, a 640x360 capture
    at 30 fps without the UI; each check asks for its own).

    Full: the TV's own portal, at the address getState reports. Slim: the receiver started with the remote
    capture, and the TV pointed at it (`instance`) and enabled; the receiver is stopped and removed at the end."""
    if receiver is None:
        portal = target.tv.wait_portal()
        reply = portal.replace_settings(_settings_document(config, capture(cell.flavour, *CAPTURE)))
        if reply.status != 200:
            pytest.fail(f'the TV portal refused the settings: HTTP {reply.status} {reply.body[:300]!r}')
        yield portal
        return
    try:
        bench.log(receiver.deliver())
        receiver.start(_settings_document(config, capture(cell.flavour, *CAPTURE)))
        instance = _instance(receiver, _tv_host(config))
        target.tv.set_preferences({'autostart': False, 'instance': instance})
        target.tv.call(commands.tv_set_enabled(target.tv.target, True))
        bench.log(f'The TV streams to {instance}')
        yield Prysm(receiver.url)
    finally:
        with uninterrupted():
            try:
                bench.log(f'Receiver stopped: exit code {receiver.stop()}')
            finally:
                receiver.close()


def _sender_plan(cell: TvCell, config: Config) -> list[str]:
    """Where the sender WLED should see comes from: the TV's portal address, this computer's route (a UDP
    connect, nothing sent) or the Linux receiver's route to WLED."""
    if cell.flavour == Flavour.FULL:
        return ['the sender WLED should see: the TV, the host of the portal address getState reports']
    machine = config.machine(cast('options.Platform', cell.receiver))
    if not machine.target.startswith(SSH):
        return ["the sender WLED should see: this computer's address on the route to WLED"]
    route = commands.linux_route(machine.target.removeprefix(SSH), '192.0.2.40')
    return [
        "the sender WLED should see: the src of the receiver target's route to WLED:",
        *shown(route),
        "(192.0.2.40: WLED's address, resolved here)",
    ]


@pytest.fixture(scope='module')
@planned(does=_sender_plan)
def running(bench: Bench, target: OnTv, receiver: Host | None, prysm: Prysm) -> Sender:
    """What lights WLED, as the shared checks see it: the address WLED should report, and the logs a failure
    fetches (the TV worker's recent lines from getState, `tv-worker.log`, and the receiver's)."""
    if receiver is None:
        return Lit(target.tv, urlsplit(prysm.base).hostname or '', bench.out_dir)
    return Lit(target.tv, receiver.sender(), bench.out_dir, receiver)


def _state_plan(wanted: Shown, config: Config, include_ui: bool = False) -> list[str]:
    """`in_state`, for the dry run."""
    ssh = _ssh(config)
    video, app = WANTED[wanted]
    width, height, fps = CAPTURE
    sets = (
        "unless the clip plays: Prysm's app closed if the runner launched it, the clip cast (screen)"
        if wanted == Shown.VIDEO
        else f'the cast stopped if it plays, then $ {commands.tv_launch_app(ssh).display()}'
    )
    size = (
        'exactly the size asked' if wanted == Shown.VIDEO else f"at most {UI_LIMIT[0]}x{UI_LIMIT[1]}, the TV's UI size"
    )
    return [
        f'the {wanted} state: {sets}; then every second for up to {CONFIRM_SECONDS} s:',
        *shown(commands.tv_get_state(ssh), commands.tv_foreground(ssh)),
        f'until power {Power.ACTIVE}, video {video}, {app} in front'
        + (
            f"; the player's overlay waited out ({config.tv.feed.settle_seconds:g} s after PLAYING)"
            if wanted == Shown.VIDEO
            else ''
        ),
        f'PUT /api/v1/settings capture {width}x{height}@{fps} ' + ('with' if include_ui else 'without') + ' the UI, '
        'PUT /api/v1/target processed, running with frames within 30 s',
        f'GET /api/v1/preview/stream: the frames that arrive are {size}',
        '(any part not reached fails the check, saying which)',
    ]


@pytest.fixture(scope='module')
@planned(does=lambda cell, config: [f'$ {host_cells.clip_command().display()}'])
def clip(bench: Bench, prysm: Prysm) -> Clip:
    """The host cells' test clip: generated, its edge colours stepping round the corners every 2 s, for 30
    minutes (longer than the cell, so it never ends or loops)."""
    return host_cells.generated(bench)


def _screen_plan(cell: TvCell, config: Config) -> list[str]:
    return [
        'nothing yet: each check sets the state it needs. The video state casts:',
        *host_cells.feed_plan(feed.on_tv(config.tv)),
    ]


def _screen_undo(cell: TvCell, config: Config) -> list[str]:
    ssh = _ssh(config)
    return [
        *host_cells.feed_undone(feed.on_tv(config.tv)),
        f'when the runner launched it: $ {commands.tv_close_app(ssh).display()}',
        f'$ {commands.tv_foreground(ssh).display()}   (saved as tv-end.json; the app found in front at the start is '
        'not brought back)',
    ]


@pytest.fixture(scope='module')
@planned(does=_screen_plan, undoes=_screen_undo)
def screen(config: Config, bench: Bench, target: OnTv, running: Sender, prysm: Prysm, clip: Clip) -> Iterator[Screen]:
    """What the TV shows, set and confirmed by each check that measures (`in_state`); at the end the cast
    stopped and Prysm's app closed, also after a failure or Ctrl-C, before the receiver stops and the package
    is uninstalled. It takes `running`, so a failure of any check in a state fetches the TV worker's log.

    The TV's first cast from this computer waits for a person to accept it on the TV: without PLAYING in time
    the runner asks on this terminal; a skip (or no terminal), or a cast that fails, fails every check that
    needs video, saying so, and the others run. Slim: the receiver's Prysm is the one sampled."""
    found = Screen(target.tv, bench, feed.on_tv(config.tv), clip)
    try:
        yield found
    finally:
        with uninterrupted():
            left = found.close()
        if left:
            pytest.fail(f'the TV was not left clean: {left}')


# ---- the checks, in order: the video state first, then the UI's -------------------


@planned(does=lambda cell, config: shown(commands.tv_get_state(_ssh(config)), commands.tv_paths(_ssh(config))))
def test_registered(cell: TvCell, target: OnTv, package: LunaReply, record: Record) -> None:
    """The service is registered: getState answers as this package with its worker; the app, service,
    manifests and boot hook are there."""
    state = target.tv.wait_state(lambda s: s['worker']['phase'] in ('running', 'stopped', 'retrying'), 20)
    if state is None or state['flavor'] != cell.flavour:
        raise Failed(f'getState does not answer as {cell.flavour}', state)
    present = target.tv.paths()
    missing = [path for path in commands.INSTALLED_PATHS if not present.get(path)]
    if missing:
        raise Failed(f'missing after setup: {missing}', present)
    record(f'registered; worker {state["worker"]["phase"]}; all files present', {'state': state, 'paths': present})


def _judged_plan(cell: TvCell, config: Config, include_ui: bool) -> list[str]:
    return [
        *_state_plan(Shown.VIDEO, config, include_ui),
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


@planned(does=lambda cell, config: _judged_plan(cell, config, include_ui=False))
def test_video_only(cell: TvCell, config: Config, screen: Screen, prysm: Prysm, record: Record) -> None:
    """Video without the UI: the clip plays with the overlay over its bottom, the capture leaves the UI out.

    The frames arrive at the size asked, at the rate within the tolerance, and the colours follow the clip on
    every edge (the host cells' colour judgement): the overlay is not in the picture."""
    _video_judged(cell, config, screen, prysm, False, record)


@planned(does=lambda cell, config: _judged_plan(cell, config, include_ui=True))
def test_video_with_overlay(cell: TvCell, config: Config, screen: Screen, prysm: Prysm, record: Record) -> None:
    """Video with the UI: the same state, only the capture now includes the UI.

    The same size and rate; the colours still follow the clip on the top edge and the sides' upper halves, and
    the bottom edge shows the overlay (white) at every step: the overlay is in the picture where it is, and
    nowhere else."""
    _video_judged(cell, config, screen, prysm, True, record)


def _video_section(cell: TvCell) -> JsonObject:
    return capture(cell.flavour, *CAPTURE)


@planned(does=lambda cell, config: [*_state_plan(Shown.VIDEO, config), f'GET http://{config.wled}/json/si'])
def test_wled_live(cell: TvCell, screen: Screen, prysm: Prysm, wled: Wled, running: Sender, record: Record) -> None:
    """In the video state, Screen sync lights WLED: live DDP from the expected sender (the TV, or the slim
    package's receiver), no output_issue (the host cells' check)."""
    reached = in_state(screen, Shown.VIDEO, prysm, _video_section(cell))
    with stated(reached):
        si = host_cells.live_from_host(prysm, wled, running)
    record(f'live DDP from {si["info"]["lip"]}; {described(reached)}', {'state': reached, 'wled': si['info']})


@planned(
    does=lambda cell, config: [
        *_state_plan(Shown.VIDEO, config),
        f'GET /api/v1/state every second for {config.checks.seconds} s',
    ]
)
def test_fps(cell: TvCell, config: Config, screen: Screen, prysm: Prysm, record: Record) -> None:
    """In the video state, with the frames at the size asked: the input's fps within the tolerance of the
    capture's, sent fps alike, never behind (the host cells' check)."""
    reached = in_state(screen, Shown.VIDEO, prysm, _video_section(cell))
    with stated(reached):
        measured = host_cells.frame_rate(prysm, config)
    record(f'{host_cells.rate_detail(measured)}; {described(reached)}', {'state': reached, 'rates': measured})


@planned(does=lambda cell, config: [*_state_plan(Shown.VIDEO, config), *shown(commands.tv_get_state(_ssh(config)))])
def test_capturing(cell: TvCell, screen: Screen, target: OnTv, prysm: Prysm, record: Record) -> None:
    """In the video state, the TV says it captures: getState capturing with a frame rate."""
    reached = in_state(screen, Shown.VIDEO, prysm, _video_section(cell))
    state = target.tv.wait_state(lambda s: s['capturing'] and (s.get('fps') or 0) > 0, 10)
    if state is None:
        raise Failed('getState does not say capturing', {'state': reached, 'tv': target.tv.get_state()})
    record(f'capturing at {state.get("fps")} fps; {described(reached)}', {'state': reached, 'tv': state})


@only(Flavour.FULL)
@planned(does=lambda cell, config: [*_state_plan(Shown.VIDEO, config), 'POST /api/v1/capture/modes {"kind":"webos"}'])
def test_capture_modes(cell: TvCell, screen: Screen, prysm: Prysm, record: Record) -> None:
    """The full package, in the video state: the TV's capture modes are known."""
    reached = in_state(screen, Shown.VIDEO, prysm, _video_section(cell))
    reply = prysm.modes({'kind': 'webos'})
    answer = prysm_api.modes_answer(reply)
    if reply.status != 200 or answer.get('status') != 'known':
        raise Failed(f'capture modes: HTTP {reply.status}', {'state': reached, 'answer': answer})
    record('; '.join(prysm_api.describe_modes(answer['modes'])), {'state': reached, 'answer': answer})


@only(Flavour.SLIM)
@planned(
    does=lambda cell, config: [
        *_state_plan(Shown.VIDEO, config),
        'PUT /api/v1/target off',
        'POST /api/v1/capture/modes {"kind":"remote","encoding":E} for each encoding until known (30 s)',
    ]
)
def test_offer(cell: TvCell, screen: Screen, prysm: Prysm, record: Record) -> None:
    """The slim package, in the video state: with the lights off, the TV's offer is known for each encoding."""
    reached = in_state(screen, Shown.VIDEO, prysm, _video_section(cell))
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
    does=lambda cell, config: [
        *_state_plan(Shown.VIDEO, config),
        f'for each encoding: PUT /api/v1/settings remote capture {CAPTURE[0]}x{CAPTURE[1]}@{CAPTURE[2]}, PUT '
        f'/api/v1/target processed, GET /api/v1/state every second for {config.checks.seconds} s, '
        'GET /api/v1/preview/stream (the size that arrives)',
        *shown(commands.tv_sample_worker(_ssh(config)), commands.tv_get_state(_ssh(config))),
        'PUT /api/v1/target off',
    ]
)
def test_encodings(cell: TvCell, config: Config, screen: Screen, target: OnTv, prysm: Prysm, record: Record) -> None:
    """The slim package, in the video state: each encoding streams at 30 fps, and getState names it; the TV
    worker's CPU.

    Raw has no rate control: its frame rate is what the link to the receiver carries. So the evidence also
    holds, per encoding, the size of the frames that arrived and the TV's power and video state, to tell a slow
    link from a larger frame."""
    reached = in_state(screen, Shown.VIDEO, prysm, _video_section(cell))
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
        first = target.tv.sample_worker()
        measured = rates(prysm.sample(config.checks.seconds))
        measured['tv_cpu'] = tv.cpu_percent(first, target.tv.sample_worker())
        state = target.tv.get_state() or LunaReply()
        measured['tv_encoding'] = state.get('encoding')
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
    does=lambda cell, config: [
        *_state_plan(Shown.VIDEO, config),
        'PUT /api/v1/target off',
        *shown(commands.tv_get_state(_ssh(config))),
        'PUT /api/v1/target processed',
    ]
)
def test_pause_resume(cell: TvCell, screen: Screen, target: OnTv, prysm: Prysm, record: Record) -> None:
    """The slim package, in the video state: the receiver's lights off pause the TV's capture; on again resumes
    it."""
    reached = in_state(screen, Shown.VIDEO, prysm, _video_section(cell))
    prysm.set_target(Target.OFF)
    start = time.monotonic()
    paused = target.tv.wait_state(lambda s: s['paused'] and not s['capturing'], 15)
    paused_after = round(time.monotonic() - start, 1)
    prysm.set_target(Target.PROCESSED)
    start = time.monotonic()
    resumed = target.tv.wait_state(lambda s: s['capturing'] and not s['paused'], 20)
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


def _soak_plan(cell: TvCell, config: Config) -> list[str]:
    sizes = ', '.join(f'{w}x{h}@{f}' for w, h, f in SOAKS[cell.flavour])
    return [
        f'for {sizes}: the video state at that size, as below (the frames that arrive must be of that size), then '
        f'{config.tv.soak_seconds} s of GET /api/v1/state every second and, every 5 s:',
        *shown(commands.tv_sample_worker(_ssh(config))),
        *_state_plan(Shown.VIDEO, config),
        f'PUT /api/v1/settings capture {CAPTURE[0]}x{CAPTURE[1]}@{CAPTURE[2]} again',
    ]


@planned(does=_soak_plan)
def test_soak(cell: TvCell, config: Config, screen: Screen, target: OnTv, prysm: Prysm, record: Record) -> None:
    """The soak, in the video state: lighting for `[tv] soak_seconds` at each size (full: 640x360 and 1280x720;
    slim: 640x360, hardware H.264) with the frames arriving at that size and the frame rate within the
    tolerance; the TV worker's CPU and RSS from /proc."""
    results: list[tv.Soak] = []
    states: list[Reached] = []
    try:
        for width, height, fps in SOAKS[cell.flavour]:
            states.append(in_state(screen, Shown.VIDEO, prysm, capture(cell.flavour, width, height, fps)))
            result = target.tv.soak(prysm, config.tv.soak_seconds)
            result['size'] = f'{width}x{height}@{fps}'
            results.append(result)
            if not near(result['fps_mean'], fps, config.checks.fps_tolerance):
                raise Failed(
                    f'{result["fps_mean"]} fps over the soak at {result["size"]}, expected {fps}',
                    {'states': states, 'soaks': results},
                )
    finally:
        ask_for(prysm, _video_section(cell))
    record('; '.join(f'{r["size"]}: {tv.soak_summary(r)}' for r in results), {'states': states, 'soaks': results})


@planned(
    does=lambda cell, config: [
        *_state_plan(Shown.VIDEO, config),
        *_install_plan(_ssh(config), str(_ipk(cell, config) or 'IPK')),
        '(getState until the worker runs, 60 s; full: GET PORTAL/api/v1/state); the video state confirmed again',
    ]
)
def test_reinstall(cell: TvCell, config: Config, screen: Screen, target: OnTv, prysm: Prysm, record: Record) -> None:
    """Reinstall while lighting, in the video state: the same package installed again keeps the preferences (and
    the full package's settings), lighting or the stream resumes by itself, and the TV still shows the video."""
    reached = in_state(screen, Shown.VIDEO, prysm, _video_section(cell))
    before = cast('LunaReply', target.tv.get_state())
    settings = prysm.state()['settings']
    target.tv.install(str(_ipk(cell, config)))
    after = target.tv.wait_state(lambda s: s['worker']['phase'] == 'running', 60)
    evidence: dict[str, object] = {'state': reached, 'before': before, 'after': after}
    if after is None:
        raise Failed('the worker is not running 60 s after the reinstall and setup', evidence)
    if after['preferences'] != before['preferences']:
        raise Failed('preferences changed by the reinstall', evidence)
    if cell.flavour == Flavour.FULL and target.tv.wait_portal().state()['settings'] != settings:
        raise Failed('settings.json changed by the reinstall', evidence)
    if prysm.wait(prysm_api.running, 30) is None:
        raise Failed('lighting did not resume after the reinstall', evidence)
    evidence['state_after'] = screen.confirm(Shown.VIDEO)
    record(f'preferences (and settings) kept, lighting resumed; {described(reached)}', evidence)


def _ui_section(cell: TvCell, include_ui: bool = True) -> JsonObject:
    return capture(cell.flavour, *CAPTURE, include_ui=include_ui)


@planned(
    does=lambda cell, config: [
        *_state_plan(Shown.UI, config, include_ui=True),
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
    the preview without a frame) and nothing failing (no failure in Prysm's state or the TV's getState, the
    session not faulted); the samples as evidence, Failed when one did."""
    samples: list[dict[str, object]] = []
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        state = prysm.state()
        tv_state = found.get_state() or LunaReply()
        samples.append(
            {
                'session': prysm_api.session(state),
                'running': prysm_api.running(state),
                'input': state.get('input'),
                'failure': state.get('failure'),
                'tv_failure': tv_state.get('failure'),
                'tv_capturing': tv_state.get('capturing'),
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
    does=lambda cell, config: [
        'the UI state, as above, without frames to wait for:',
        *_state_plan(Shown.UI, config, include_ui=False)[:4],
        f'PUT /api/v1/target processed; for {QUIET_SECONDS} s every second GET /api/v1/state and:',
        *shown(commands.tv_get_state(_ssh(config))),
        'GET /api/v1/preview/stream: no frame',
        'then the capture with the UI again: running with frames within 30 s, of the UI size',
    ]
)
def test_ui_off(cell: TvCell, screen: Screen, target: OnTv, prysm: Prysm, record: Record) -> None:
    """No video and no UI: the same state, the capture without the UI: no frames, by design, and nothing fails
    (no failure in Prysm's state or the TV's, the session not faulted) for 10 s; with the UI on again the frames
    return, of the UI size."""
    screen.show(Shown.UI)
    reached = screen.confirm(Shown.UI)
    with stated(reached):
        _capture_saved(prysm, _ui_section(cell, include_ui=False))
        reply = prysm.set_target(Target.PROCESSED)
        if reply.status != 204:
            raise Failed(f'PUT /api/v1/target processed: HTTP {reply.status}', reply.json())
        quiet = _quiet(prysm, target.tv, QUIET_SECONDS)
    back = in_state(screen, Shown.UI, prysm, _ui_section(cell))
    record(
        f'no frames and no failure for {QUIET_SECONDS} s; frames again with the UI: {described(back)}',
        {'state': reached, 'quiet': quiet, 'again': back},
    )


@planned(
    does=lambda cell, config: [
        *_state_plan(Shown.UI, config, include_ui=True),
        'a person puts the TV in standby with the remote, then wakes it',
        *shown(commands.tv_get_state(_ssh(config))),
        '(every 3 s: standby seen if the TV answers, then active, worker running and capturing within 90 s)',
    ]
)
def test_standby(cell: TvCell, screen: Screen, target: OnTv, prysm: Prysm, record: Record) -> None:
    """Standby and wake (a person presses power), from the UI state: getState shows standby if the TV answers,
    then active with the worker running and capturing within 90 s of waking. The runner never puts the TV to
    sleep. Last: it ends the states the other checks set."""
    reached = in_state(screen, Shown.UI, prysm, _ui_section(cell))
    answer, note = ask_person(
        "Put the TV in standby with the remote's power button (do not unplug). Answer y once the screen is off.",
        (Answer.YES, Answer.SKIP),
    )
    if answer != Answer.YES:
        pytest.skip(f'standby skipped {note}'.strip())
    asleep = target.tv.wait_state(lambda s: s['power'] != Power.ACTIVE, 60, every=3)
    evidence: dict[str, object] = {'state': reached, 'standby_state': asleep}
    answer, note = ask_person(
        'Wake the TV with the power button. Answer y when the picture is back.',
        (Answer.YES, Answer.SKIP),
    )
    if answer != Answer.YES:
        pytest.skip(f'wake skipped {note}'.strip())
    awake = target.tv.wait_state(
        lambda s: s['power'] == Power.ACTIVE and s['worker']['phase'] == 'running' and s['capturing'], 90, every=3
    )
    evidence['wake_state'] = awake
    if awake is None:
        raise Failed('capture did not resume within 90 s of waking', evidence)
    record(f'standby seen: {asleep["power"] if asleep else "no answer"}; capture resumed', evidence)
