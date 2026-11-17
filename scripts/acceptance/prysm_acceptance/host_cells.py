"""The host cells: Prysm delivered to a platform, driving WLED from the generated clip or a capture card.

`acceptance host` runs this module with pytest and the `bench` plugin; a cell
is the `cell` parameter (`mac-binary-file`, `linux-binary-device`, `linux-deb-device` and so on).
Its fixtures set up, in order: the artifact and tools, the target (the seam
in `host`, and whether it can run the cell), WLED's snapshot, a Prysm already
installed replaced (the package cell with `[linux] installed = "replace"`),
the clip, the feed into the capture card, Prysm delivered and running, and its API;
the checks follow in this module's order and the teardowns undo it in
reverse. Each fixture and check says what it does in its docstring and, for
the dry run, in `planned`.
"""

from collections.abc import Callable, Generator, Iterator
import contextlib
from dataclasses import dataclass
from pathlib import Path
import shutil
import time
from typing import assert_never, cast
import warnings

import pytest

from prysm_acceptance import artifact as artifacts
from prysm_acceptance import (
    colour_page,
    commands,
    edges_clip,
    feed,
    host,
    host_deb,
    host_image,
    installed,
    options,
    prysm_api,
    transport,
    wled_api,
)
from prysm_acceptance.artifact import Artifact
from prysm_acceptance.bench import AnyCell, Planner, only, unavailable, uninterrupted, warn_at_end
from prysm_acceptance.checks import Answer, Bench, Failed, Skipped, ask_person
from prysm_acceptance.config import SSH, Config, Installed, Machine
from prysm_acceptance.edges_clip import CLIP
from prysm_acceptance.feed import Feed
from prysm_acceptance.host import Host, Sender
from prysm_acceptance.options import Cell, Delivery, Source
from prysm_acceptance.prysm_api import ColorsEvent, Prysm, SessionState, Target, near, rates, session
from prysm_acceptance.web import JsonObject
from prysm_acceptance.wled_api import Wled

CELLS = list(options.CELLS)
planned = Planner[Cell]()
# The `record` fixture: a passed check's detail and evidence.
Record = Callable[[str, object], None]


@dataclass(frozen=True)
class Clip:
    path: Path
    edges: bool  # the generated clip, so the colour check knows its edges


def _machine(cell: Cell, config: Config) -> Machine:
    return config.machine(cell.platform)


def _ssh_host(cell: Cell, config: Config) -> str:
    """The platform's target as ssh names it (USER@HOST)."""
    return _machine(cell, config).target.removeprefix(SSH)


def _feed(cell: Cell, config: Config, request: pytest.FixtureRequest | None = None) -> Feed | None:
    """The feed this cell shows the clip on: only a capture card has an input, and not with --display-check."""
    display_check = request is not None and bool(request.config.getoption('display_check'))
    if cell.source != Source.DEVICE or display_check:
        return None
    return feed.configured(_machine(cell, config))


def clip_command(path: str = 'RUN_DIR/CELL/edges.mp4') -> commands.Command:
    """The generated clip's FFmpeg command (the TV cells' too)."""
    return commands.ffmpeg_edges_clip(
        path,
        CLIP['width'],
        CLIP['height'],
        CLIP['fps'],
        CLIP['seconds'],
        edges_clip.corner_hex(),
        edges_clip.STEP_SECONDS,
    )


def _local_only(cell: Cell, config: Config) -> str | None:
    """Why this cell cannot run on a local target: the package and the image run over SSH only."""
    if cell.delivery != Delivery.BINARY and not _machine(cell, config).target.startswith(SSH):
        return (
            f'the {cell.delivery} cell needs [{cell.platform}] target = "ssh://USER@HOST" ("local" is not implemented)'
        )
    return None


def _seam(cell: Cell, config: Config, run: transport.Transport, out_dir: Path) -> Host:
    """The host seam of this cell's delivery (the package and the image over SSH only: see `_local_only`)."""
    machine = _machine(cell, config)
    artifact = machine.artifact(cell.delivery) or Path(str(cell.delivery).upper())
    target = _ssh_host(cell, config)
    match cell.delivery:
        case Delivery.BINARY:
            if machine.target.startswith(SSH):
                return host.SshBinary(run, target, artifact, machine.listen, out_dir, config.wled)
            return host.LocalBinary(run, artifact, machine.listen, out_dir, config.wled)
        case Delivery.DEB:
            return host_deb.SshDeb(run, target, artifact, machine.listen, out_dir, config.wled, machine.installed)
        case Delivery.IMAGE:
            return host_image.SshImage(run, target, artifact, out_dir, config.wled)
        case _:
            assert_never(cell.delivery)


# ---- fixtures, in the order they set up ------------------------------------


@pytest.fixture(scope='module')
@planned(
    does=lambda cell, config: [
        f'[{cell.platform}] {cell.delivery}: {_machine(cell, config).artifact(cell.delivery) or "(not set)"}; its '
        'SHA-256 and age, and the version and commit from manifest.json next to it when the hash matches',
        f'$ {commands.git_last_commit_time().display()}   (warns when the artifact is older)',
        *(['FFmpeg on PATH for the clip'] if cell.source == Source.FILE or _feed(cell, config) else []),
        *([f'[{cell.platform}] device set'] if cell.source == Source.DEVICE else []),
    ]
)
def artifact(cell: Cell, config: Config, bench: Bench, request: pytest.FixtureRequest) -> Artifact:
    """The configuration, the tools and the artifact under test.

    A missing artifact, tool or device skips the cell, or fails it when it was named."""
    if not config.wled:
        pytest.fail('wled.address is not set (config file or --wled)', pytrace=False)
    if sum(config.leds.as_settings().values()) == 0:
        pytest.fail('leds: no LEDs configured', pytrace=False)
    machine = _machine(cell, config)
    path = machine.artifact(cell.delivery)
    if path is None or not path.is_file():
        unavailable(request, cell, f'no artifact: [{cell.platform}] {cell.delivery} is {path or "not set"}')
    local = _local_only(cell, config)
    if local:
        unavailable(request, cell, local)
    if cell.source == Source.DEVICE and not machine.device:
        unavailable(request, cell, f'no capture device: [{cell.platform}] device is not set')
    controlled = _feed(cell, config, request)
    tools(request, cell, controlled, clip=cell.source == Source.FILE or controlled is not None)
    found = artifacts.inspect(path)
    facts = found.facts()
    warning = artifacts.older_than_checkout(found, bench.transport)
    if warning:
        facts['warning'] = warning
        warnings.warn(warning, stacklevel=1)
    bench.save('artifact.json', facts)
    bench.log('Artifact: ' + ', '.join(f'{key} {value}' for key, value in facts.items()))
    return found


def tools(request: pytest.FixtureRequest, cell: AnyCell, controlled: Feed | None, clip: bool) -> None:
    """The feed's tools, and FFmpeg when the cell generates the clip; what is missing makes the cell unavailable
    (the TV cells' too)."""
    if controlled:
        try:
            controlled.preflight()
        except Failed as error:
            unavailable(request, cell, str(error))
    if clip and not shutil.which('ffmpeg'):
        unavailable(request, cell, 'the test clip needs FFmpeg 9+ on PATH')


def _plan(cell: Cell, config: Config) -> host.Plan:
    """The seam's plan for the dry run, with nothing run."""
    local = _local_only(cell, config)
    if local:
        return host.Plan(preflight=[f'skipped: {local}'])
    seam = _seam(cell, config, transport.Refusing(), Path('RUN_DIR/CELL'))
    return seam.plan(cell.source, _machine(cell, config).device)


@pytest.fixture(scope='module')
@planned(
    does=lambda cell, config: _plan(cell, config).preflight,
)
def target(cell: Cell, config: Config, bench: Bench, request: pytest.FixtureRequest, artifact: Artifact) -> Host:
    """Where Prysm runs, and whether it can run this cell there.

    Over SSH: reachable without a prompt, Linux on the artifact's machine, FFmpeg 9+ (file) or the device
    readable (device), the listen port free. What fails skips the cell, or fails it when it was named."""
    seam = _seam(cell, config, bench.transport, bench.out_dir)
    try:
        reason = seam.preflight(cell.source, _machine(cell, config).device)
    except transport.CommandFailed as error:
        reason = f'the target preflight failed: {error}'
    if reason:
        unavailable(request, cell, reason)
    return seam


def wled_saved(cell: object, config: Config) -> list[str]:
    """The `wled` fixture's set-up, for the dry run (the TV cells' too)."""
    return [
        f'GET http://{config.wled}/json/si and /json/cfg: saved (wled-snapshot.json, the configuration as '
        'evidence in wled-cfg.json); refuses while something else drives WLED, before anything changes'
    ]


def wled_restored(cell: object, config: Config) -> list[str]:
    """The `wled` fixture's teardown, for the dry run (the TV cells' too)."""
    return [
        'WLED restored from wled-snapshot.json (live off, preset, on, bri, lor, transition, if.live.maxbri) and '
        'compared; by hand: acceptance restore-wled RUN_DIR/CELL/wled-snapshot.json'
    ]


@contextlib.contextmanager
def wled_as_found(config: Config, bench: Bench) -> Generator[Wled, None, None]:
    """WLED saved, refused while something else drives it; put back and compared at the end (the `wled`
    fixture of every cell)."""
    client = Wled(config.wled)
    saved = wled_api.snapshot(client)
    if saved['info'].get('live'):
        # Two senders would fight over the LEDs and no check could be trusted; and the runner never switches
        # someone's lighting off.
        pytest.fail(f'WLED is already driven by {saved["info"].get("lip")}: turn that lighting off first')
    path = bench.save('wled-snapshot.json', saved)
    bench.save('wled-cfg.json', wled_api.cfg(client.cfg_raw()))
    yield client
    with uninterrupted():
        left = wled_api.restore(client, saved)
    if left:
        pytest.fail(f'WLED still differs from the snapshot: {left} (acceptance restore-wled {path})')


@pytest.fixture(scope='module')
@planned(does=wled_saved, undoes=wled_restored)
def wled(config: Config, bench: Bench, target: Host) -> Iterator[Wled]:
    """WLED as found, saved before anything changes and put back last, also after a failure or Ctrl-C."""
    with wled_as_found(config, bench) as client:
        yield client


def replaced(existing: str, policy: Installed, steps: list[str]) -> list[str]:
    """What `installed = "replace"` does on a target, for the dry run (the TV cells' too)."""
    if policy == Installed.REFUSE:
        return [f'nothing: a Prysm on the {existing} target stops the preflight ([{existing}] installed = "refuse")']
    return [
        f'when the preflight found Prysm on the {existing} target: its settings read through its API, saved as '
        f'RUN_DIR/CELL/{installed.BACKUP}, read back and compared (else it stops: nothing removed); the end of the '
        'run then warns that Prysm was removed and not reinstalled, with the restore command',
        *steps,
    ]


def _replaced(cell: Cell, config: Config) -> list[str]:
    if cell.delivery != Delivery.DEB:
        return []
    target = _ssh_host(cell, config)
    existing = host_deb.DebPrysm(transport.Refusing(), target or 'USER@HOST')
    return replaced(
        'linux',
        config.linux.installed,
        [
            f'GET {existing.url}/api/v1/state (refused while an administrator unit or drop-in is under /etc)',
            *host.shown(commands.deb_purge(existing.target), commands.deb_leftovers(existing.target)),
            '(anything still listed stops the cell)',
        ],
    )


@pytest.fixture(scope='module')
@planned(does=_replaced)
def cleared(cell: Cell, bench: Bench, request: pytest.FixtureRequest, target: Host, wled: Wled) -> None:
    """With `[linux] installed = "replace"`, the package the preflight found replaced (the package cell).

    After the WLED snapshot, so Prysm lighting WLED is refused before anything is removed."""
    if not isinstance(target, host_deb.SshDeb) or not target.found:
        return
    path = bench.out_dir / installed.BACKUP
    bench.log(installed.replace(target.existing, target.found, path, lambda text: warn_at_end(request, text)))


@pytest.fixture(scope='module')
@planned(
    does=lambda cell, config: (
        [f'$ {clip_command().display()}']
        if cell.source == Source.FILE or _feed(cell, config)
        else ['none: no feed shows a clip on the capture card']
    )
)
def clip(cell: Cell, config: Config, bench: Bench, request: pytest.FixtureRequest, wled: Wled) -> Clip | None:
    """The test clip: generated, its edge colours stepping round the corners every 2 s (or --clip PATH)."""
    given = request.config.getoption('clip')
    if cell.source == Source.FILE and given:
        return Clip(Path(str(given)).resolve(), edges=False)
    if cell.source == Source.DEVICE and not _feed(cell, config, request):
        return None
    return generated(bench)


def generated(bench: Bench) -> Clip:
    """The generated clip, in the cell's run directory (the TV cells' too)."""
    path = bench.out_dir / 'edges.mp4'
    bench.run(clip_command(str(path)))
    return Clip(path, edges=True)


def feed_plan(controlled: Feed | None) -> list[str]:
    """The `feed_shown` fixture's set-up, for the dry run (the TV cells' too)."""
    if controlled is None:
        return []
    return [
        *controlled.actions(),
        *(f'$ {command.display()}' for command in controlled.commands()),
        controlled.how(),
    ]


def feed_undone(controlled: Feed | None) -> list[str]:
    """The `feed_shown` fixture's teardown, for the dry run (the TV cells' too)."""
    return list(controlled.CLEANUPS) if controlled else []


@contextlib.contextmanager
def showing(controlled: Feed, bench: Bench, clip: Clip) -> Generator[Feed, None, None]:
    """The clip shown through the feed (feed.json); stopped at the end, also after a failure or Ctrl-C (the
    `feed_shown` fixture of every cell).

    A clip the feed did not show without anything failing (`Skipped`: on the TV, nobody accepted it) skips what
    needs it; what the feed started is stopped at once."""
    try:
        try:
            detail, evidence = controlled.show(bench, str(clip.path))
        except Skipped as error:
            bench.save('feed.json', {'skipped': str(error), 'evidence': error.evidence})
            pytest.skip(str(error))
        bench.save('feed.json', {'detail': detail, 'evidence': evidence})
        yield controlled
    finally:
        with uninterrupted():
            failures = bench.undo()
        if failures:
            pytest.fail(f'the feed was not stopped cleanly: {failures}')


@pytest.fixture(scope='module')
@planned(
    does=lambda cell, config: feed_plan(_feed(cell, config)) or ['none: the card records its input'],
    undoes=lambda cell, config: feed_undone(_feed(cell, config)),
)
def feed_shown(
    cell: Cell, config: Config, bench: Bench, request: pytest.FixtureRequest, clip: Clip | None
) -> Iterator[Feed | None]:
    """The clip on the capture card's input, through the configured feed; stopped at the end."""
    controlled = _feed(cell, config, request)
    if controlled is None or clip is None:
        yield None
        return
    with showing(controlled, bench, clip):
        yield controlled


@pytest.fixture(scope='module')
@planned(
    does=lambda cell, config: [
        f'write RUN_DIR/CELL/headless-settings.json: WLED output, neutral processing, the {cell.source} capture',
        *_plan(cell, config).start,
    ],
    undoes=lambda cell, config: _plan(cell, config).stop,
)
def running(
    cell: Cell,
    config: Config,
    bench: Bench,
    target: Host,
    wled: Wled,
    cleared: None,
    clip: Clip | None,
    feed_shown: Feed | None,
) -> Iterator[Host]:
    """Prysm delivered and started with temporary settings; stopped before WLED is restored.

    Over SSH the run directory on the target is removed at the end, after its log and the kernel's USB lines
    are fetched (headless.log, or the package's journal.log, the container's container.log; kernel-usb.log);
    the package is purged, the image and its compose project removed."""
    try:
        bench.log(target.deliver())
        place = target.place(clip.path) if clip and cell.source == Source.FILE else None
        capture = capture_settings(cell, config, place, target.capture_path(_machine(cell, config).device))
        state = target.start(host.settings(host.wled_output(config), capture))
        bench.log(f'Started at {target.url}: target {state["target"]}, session {session(state)}')
        yield target
    finally:
        with uninterrupted():
            try:
                bench.log(f'Stopped: exit code {target.stop()}')
            finally:
                target.close()


def capture_settings(cell: Cell, config: Config, clip: str | None, device: str | None = None) -> JsonObject:
    """The capture section: the clip where the host reads it, or the configured device (at `device`, where the
    seam's Prysm opens it, when given) and size."""
    match cell.source:
        case Source.FILE:
            path = clip or ''
            size = {'width': CLIP['width'], 'height': CLIP['height'], 'fps': CLIP['fps']}
            return {'kind': 'file', 'path': path, **size, 'repeat': True}
        case Source.DEVICE:
            machine = _machine(cell, config)
            return {
                'kind': 'device',
                'path': device or machine.device,
                'width': machine.capture.width,
                'height': machine.capture.height,
                'fps': machine.capture.fps,
            }
        case _:
            assert_never(cell.source)


@pytest.fixture(scope='module')
@planned(
    does=lambda cell, config: (
        [
            'GET /api/v1/candidates/capture/device: the configured device is listed, a link as the node it resolves '
            'to on the target, in a container as /dev/video0 (else fails, listing the devices with their IDs)',
            'POST /api/v1/capture/modes: a mode takes the configured size and rate',
        ]
        if cell.source == Source.DEVICE
        else []
    )
)
def prysm(cell: Cell, config: Config, bench: Bench, running: Host) -> Prysm:
    """Prysm's API on the running host; a capture card is checked to be present and to offer the size."""
    client = Prysm(running.url)
    if cell.source == Source.DEVICE:
        device = _machine(cell, config).device
        bench.save('capture-device.json', device_offered(client, running.capture_path(device), running.listed(device)))
    return client


def device_offered(prysm: Prysm, device: str, listed_as: str | None = None) -> object:
    """The configured device among Prysm's candidates (as `listed_as`: a link's node), offering the capture size
    and rate in the settings."""
    found = prysm.candidates('capture', 'device')
    present = [candidate['fields'].get('path') for candidate in found]
    if (listed_as or device) not in present:
        listed = ', '.join(f'{c["label"]} ({c["fields"].get("path")})' for c in found) or 'none'
        node = f' ({listed_as})' if listed_as and listed_as != device else ''
        raise Failed(f'no capture device {device!r}{node}; present: {listed}', found)
    size = _running_size(prysm)
    reply = prysm.modes({'kind': 'device', 'path': device})
    answer = prysm_api.modes_answer(reply)
    if reply.status != 200 or answer.get('status') != 'known':
        raise Failed(f'capture modes of {device} unknown (HTTP {reply.status})', answer)
    modes = answer['modes']
    if not prysm_api.offers(modes, size):
        raise Failed(
            f'{device} offers no mode of {size["width"]}x{size["height"]} @ {size["fps"]}: '
            + '; '.join(prysm_api.describe_modes(modes)),
            answer,
        )
    return {'devices': found, 'modes': modes, 'size': size}


# ---- helpers the checks share ----------------------------------------------


def processed(prysm: Prysm, seconds: float = 15) -> prysm_api.State:
    """Screen sync on: target processed answered 204, then running with frames."""
    reply = prysm.set_target(Target.PROCESSED)
    if reply.status != 204:
        raise Failed(f'PUT /api/v1/target processed: HTTP {reply.status}', reply.json())
    state = prysm.wait(prysm_api.running, seconds)
    if state is None:
        raise Failed(f'not running with frames within {seconds} s', prysm.state())
    return state


def live_from_host(prysm: Prysm, lights: Wled, seam: Sender) -> wled_api.Si:
    """WLED reports live DDP from the sender the seam (or the TV) expects, and Prysm no output issue."""
    expected = seam.sender()
    si = lights.wait(lambda s: s['info'].get('live') and s['info'].get('lm') == 'DDP', 10)
    if si is None:
        raise Failed('WLED did not report live DDP within 10 s', lights.si()['info'])
    if si['info'].get('lip') != expected:
        raise Failed(f'WLED shows another sender: {si["info"].get("lip")} (expected {expected})', si['info'])
    issue = prysm.state().get('output_issue')
    if issue:
        raise Failed(f'Prysm reports output_issue {issue}')
    return si


def _running_size(prysm: Prysm) -> prysm_api.Size:
    """The capture size and rate in the settings Prysm runs with (the configured ones, or its defaults)."""
    capture = cast('prysm_api.Size', prysm_api.section(prysm.state()['settings'], 'capture'))
    return {'width': capture['width'], 'height': capture['height'], 'fps': capture['fps']}


def _last_colours(prysm: Prysm, seconds: float = 3.0) -> ColorsEvent:
    found = [event for event in prysm.colors(seconds) if event.get('type') == 'colors']
    if not found:
        raise Failed(f'no colours event within {seconds} s')
    return found[-1]


# ---- the checks, in order ----------------------------------------------------


@planned(does=lambda cell, config: ['GET /metrics: prysm_build_info{version}'])
def test_version(prysm: Prysm, artifact: Artifact, record: Record) -> None:
    """The running version is the artifact's.

    `prysm_build_info` in /metrics against the version in manifest.json; a skip without a matching manifest."""
    running = prysm.version()
    if artifact.version is None:
        pytest.skip(f'running {running}; no manifest.json matches {artifact.path.name}, so its version is unknown')
    if running != artifact.version:
        raise Failed(f'running {running}, the artifact is {artifact.version}', artifact.facts())
    record(f'running {running}, as built', artifact.facts())


@planned(does=lambda cell, config: ['PUT /api/v1/target processed', f'GET http://{config.wled}/json/si'])
def test_wled_live(prysm: Prysm, wled: Wled, running: Sender, record: Record) -> None:
    """Screen sync starts and WLED shows Prysm.

    Target processed is 204, the session runs with frames within 15 s; WLED reports live DDP from the sender
    the seam expects (a TV cell: the TV, or the slim package's receiver); no output_issue."""
    state = processed(prysm)
    si = live_from_host(prysm, wled, running)
    record(f'running at {(state["input"] or {}).get("fps")} fps; live DDP from {si["info"]["lip"]}', si['info'])


@planned(does=lambda cell, config: [f'GET /api/v1/state every second for {config.checks.seconds} s'])
def test_fps(prysm: Prysm, config: Config, record: Record) -> None:
    """Frame rate: the input's fps within the tolerance of the source's, sent fps alike, never behind."""
    expected = _running_size(prysm)['fps']
    tolerance = config.checks.fps_tolerance
    processed(prysm)
    measured = rates(prysm.sample(config.checks.seconds))
    measured['expected'] = expected
    if not near(measured['fps_mean'], expected, tolerance):
        raise Failed(f'input {measured["fps_mean"]} fps, expected {expected} ±{tolerance:.0%}', measured)
    if measured['sent_mean'] < expected * (1 - tolerance):
        raise Failed(f'sent {measured["sent_mean"]} fps, expected about {expected}', measured)
    if measured['behind']:
        raise Failed(f'Prysm fell behind: {measured["behind"]}', measured)
    record(f'{measured["fps_mean"]} fps in, {measured["sent_mean"]} sent (min {measured["fps_min"]})', measured)


@planned(does=lambda cell, config: [f'GET /api/v1/colors/stream for {edges_clip.CHECK_SECONDS:g} s'])
def test_colours(prysm: Prysm, clip: Clip | None, feed_shown: Feed | None, record: Record) -> None:
    """The colours follow the clip.

    Each edge's two halves are nearest the clip's two colours among nine, in the right direction; at least two
    steps, in order, 2 s apart. A clip of unknown content, or a card without a feed: not black; a TV cell without
    its feed, or with the clip not shown there: skipped."""
    processed(prysm)
    if feed_shown:
        feed_shown.settled()
    if clip and clip.edges:
        seconds = edges_clip.CHECK_SECONDS
        detail, evidence = edges_clip.judge(prysm.colors(seconds, timed=True), seconds)
        record(detail, evidence)
        return
    event = _last_colours(prysm)
    brightest = max(max(sum(c) for c in event.get(edge) or [[0, 0, 0]]) for edge in prysm_api.EDGES)
    if brightest < 30:
        raise Failed('every edge is black', event)
    record(f'not black (brightest edge sum {brightest})', event)


@planned(
    does=lambda cell, config: [
        'PUT /api/v1/target device_native, processed, off',
        f'GET http://{config.wled}/json/si after each',
    ]
)
def test_targets(prysm: Prysm, wled: Wled, running: Host, record: Record) -> None:
    """WLED effects, Screen sync again, off.

    device_native: session native, WLED on and not live; processed: live DDP from the expected sender again;
    off: session stopped, WLED off."""
    evidence: dict[str, object] = {}

    def native(si: wled_api.Si) -> object:
        return not si['info'].get('live') and si['state'].get('on')

    def off(si: wled_api.Si) -> object:
        return not si['state'].get('on')

    for target, wanted, lights in (
        (Target.DEVICE_NATIVE, SessionState.NATIVE, native),
        (Target.OFF, SessionState.STOPPED, off),
    ):
        reply = prysm.set_target(target)
        if reply.status != 204:
            raise Failed(f'PUT /api/v1/target {target}: HTTP {reply.status}', reply.json())
        state = prysm.wait(lambda s, wanted=wanted: session(s) == wanted, 10)
        si = wled.wait(lights, 10)
        if state is None or si is None:
            raise Failed(
                f'{target}: Prysm not {wanted} or WLED not as expected', {'prysm': prysm.state(), 'wled': wled.si()}
            )
        evidence[target] = si['state']
        if target == Target.DEVICE_NATIVE:
            processed(prysm)
            evidence[Target.PROCESSED] = live_from_host(prysm, wled, running)['info']
    record('native: WLED on, not live; processed: live again; off: WLED off', evidence)


@planned(
    does=lambda cell, config: [
        'POST /json/state {"bri":102}; POST /json/cfg {"if":{"live":{"maxbri":true}}}; both put back after'
    ]
)
def test_brightness(prysm: Prysm, wled: Wled, record: Record) -> None:
    """WLED's brightness reaches Prysm (WLED 16.0.1, prysm-3jx.74).

    bri 102: output_brightness 40 % within 5 s; Force max brightness on: back to 100 % within 35 s with
    state.bri unchanged; /json/cfg under 64 KiB."""
    raw = wled.cfg_raw()
    found = wled_api.cfg(raw)
    evidence: dict[str, object] = {
        'cfg_bytes': len(raw),
        'maxbri': wled_api.lookup(found, 'if', 'live', 'maxbri'),
        'scale_bri': wled_api.lookup(found, 'light', 'scale-bri'),
    }
    if len(raw) >= 65536:
        raise Failed('/json/cfg is 64 KiB or more', evidence)
    if evidence['maxbri'] is None or evidence['scale_bri'] is None:
        raise Failed('/json/cfg lacks if.live.maxbri or light.scale-bri', evidence)
    processed(prysm)
    bri = wled.si()['state'].get('bri', 128)
    try:
        wled.post_state({'bri': 102})
        start = time.monotonic()
        state = prysm.wait(lambda s: abs((s.get('output_brightness') or {}).get('percent', -99) - 40) <= 2, 5)
        evidence['note_40_seconds'] = round(time.monotonic() - start, 1) if state else None
        if state is None:
            raise Failed('output_brightness did not report 40 % within 5 s', evidence)
        wled.post_cfg({'if': {'live': {'maxbri': True}}})
        start = time.monotonic()
        state = prysm.wait(lambda s: (s.get('output_brightness') or {'percent': 100}).get('percent') == 100, 35)
        evidence['force_max_seconds'] = round(time.monotonic() - start, 1) if state else None
        evidence['bri_with_force_max'] = wled.si()['state'].get('bri')
        if state is None:
            raise Failed('the note stayed with Force max brightness on (35 s)', evidence)
        if evidence['bri_with_force_max'] != 102:
            raise Failed('WLED state.bri changed under live mode', evidence)
    finally:
        wled.post_cfg({'if': {'live': {'maxbri': evidence['maxbri']}}})
        wled.post_state({'bri': bri})
    record(
        f'40 % after {evidence["note_40_seconds"]} s, gone {evidence["force_max_seconds"]} s after Force max', evidence
    )


@planned(does=lambda cell, config: ['PUT /api/v1/target off, then processed'] if cell.source == Source.DEVICE else [])
def test_release(cell: Cell, prysm: Prysm, config: Config, feed_shown: Feed | None, record: Record) -> None:
    """Capture release: off, then processed; the device opens again with frames (a capture card only)."""
    if cell.source != Source.DEVICE:
        pytest.skip('a capture card only')
    reply = prysm.set_target(Target.OFF)
    if reply.status != 204 or prysm.wait(lambda s: session(s) == SessionState.STOPPED, 10) is None:
        raise Failed('Prysm did not stop', prysm.state())
    state = processed(prysm, seconds=20)
    measured = rates(prysm.sample(config.checks.seconds))
    if measured['fps_mean'] <= 0:
        raise Failed('no frames after re-opening', measured)
    if feed_shown:
        feed_shown.settled()
    record(f're-opened: {measured["fps_mean"]} fps', {'first': state['input'], 'rates': measured})


@planned(
    does=lambda cell, config: (
        [f'serve http://127.0.0.1:{_machine(cell, config).display_port}/ (localhost only); a person opens it']
        if cell.source == Source.DEVICE
        else []
    )
)
def test_display_check(
    cell: Cell, config: Config, prysm: Prysm, request: pytest.FixtureRequest, record: Record
) -> None:
    """Colour fields on the capture display (--display-check, a person sets it up).

    When the card records this Mac's HDMI output, a local page shows red, green, blue, white and an edge
    pattern full screen; each edge (middle half) is nearest the field's colour among nine."""
    if cell.source != Source.DEVICE or not request.config.getoption('display_check'):
        pytest.skip("needs --source device --display-check with this Mac as the card's input")
    page = colour_page.ColourPage(_machine(cell, config).display_port).start()
    try:
        answer, note = ask_person(
            f'Open {page.url} in a browser on the display the capture card records '
            "(this Mac's HDMI output), press F for full screen. Ready?",
            (Answer.YES, Answer.SKIP),
        )
        if answer != Answer.YES:
            pytest.skip(f'operator skipped the display set-up {note}'.strip())
        processed(prysm)
        results: dict[str, object] = {}
        ok = True
        for name, edges in colour_page.FIELDS:
            if not page.show(name, edges):
                raise Failed(f'the page did not show {name} within 10 s (is it open?)', results)
            time.sleep(1.5)
            matched, means = colour_page.matches(_last_colours(prysm, 2.0), edges)
            results[name] = {
                'ok': matched,
                'measured': means,
                'expected': edges,
                'full_screen': (page.shown or {}).get('full'),
            }
            ok = ok and matched
        page.show('', None, 2)
        if not ok:
            raise Failed('some fields differ beyond the tolerance', results)
        record(f'{len(results)} fields, each edge the expected colour', results)
    finally:
        page.close()


# ---- what a delivery adds: each check is collected only for its delivery's cell (`only`) ----


def _settings_kept(prysm: Prysm, wled: Wled, seam: Host, renew: Callable[[], prysm_api.State]) -> object:
    """While lighting, `renew` Prysm (a unit restart, a new container): it answers idle with the settings it had,
    and Screen sync lights WLED from the expected sender again."""
    processed(prysm)
    saved = prysm.state()['settings']
    state = renew()
    evidence: dict[str, object] = {'target': state['target'], 'session': session(state)}
    if state['target'] != Target.OFF or session(state) != SessionState.STOPPED:
        raise Failed(f'not idle after the restart: target {state["target"]}, session {session(state)}', evidence)
    if state['settings'] != saved:
        raise Failed('the settings did not come back', {**evidence, 'before': saved, 'after': state['settings']})
    processed(prysm)
    evidence['wled'] = live_from_host(prysm, wled, seam)['info']
    return evidence


@only(Delivery.DEB)
@planned(does=lambda cell, config: [f'$ {commands.deb_main_user(_ssh_host(cell, config)).display()}'])
def test_package_user(running: Host, record: Record) -> None:
    """The Debian package: the unit's main process does not run as root."""
    user = cast('host_deb.SshDeb', running).main_user()
    if user in ('', 'root'):
        raise Failed(f'the unit runs as {user or "nobody (no main process)"}')
    record(f'runs as {user}', {'user': user})


@only(Delivery.DEB)
@planned(
    does=lambda cell, config: [
        'PUT /api/v1/target processed',
        f'$ {commands.deb_systemctl(_ssh_host(cell, config), commands.UnitAction.RESTART).display()}',
        'GET /api/v1/state until it answers, then PUT /api/v1/target processed',
    ]
)
def test_package_restart(prysm: Prysm, wled: Wled, running: Host, record: Record) -> None:
    """The Debian package: a restart of the unit while lighting.

    Active again, idle (as the package documents: a restart leaves the lights off) with the saved settings;
    Screen sync then lights WLED again."""
    seam = cast('host_deb.SshDeb', running)
    record('idle with its settings after the restart; lighting again', _settings_kept(prysm, wled, seam, seam.restart))


@only(Delivery.IMAGE)
@planned(
    does=lambda cell, config: [
        'PUT /api/v1/target processed',
        '$ '
        + commands.image_compose(_ssh_host(cell, config), 'RUN_DIR-CELL', commands.ComposeAction.RECREATE).display(),
        'GET /api/v1/state until it answers, then PUT /api/v1/target processed',
    ]
)
def test_image_volume(prysm: Prysm, wled: Wled, running: Host, record: Record) -> None:
    """The image: the settings live on the compose file's volume.

    A new container (`up --force-recreate`) answers idle with the same settings; Screen sync then lights WLED
    again."""
    seam = cast('host_image.SshImage', running)
    record('a new container has the settings; lighting again', _settings_kept(prysm, wled, seam, seam.recreate))


# Last: it stops the host the checks above share.
@planned(does=lambda cell, config: ['PUT /api/v1/target processed', 'SIGTERM to Prysm'])
def test_sigterm(prysm: Prysm, wled: Wled, running: Host, record: Record) -> None:
    """Clean exit: SIGTERM while lighting, exit code 0 and WLED released."""
    processed(prysm)
    start = time.monotonic()
    code = running.stop()
    seconds = round(time.monotonic() - start, 1)
    if code != 0:
        raise Failed(f'exit code {code} after SIGTERM', {'seconds': seconds, 'log': running.logs()})
    si = wled.wait(lambda s: not s['info'].get('live'), 10)
    if si is None:
        raise Failed('WLED still live after Prysm exited', wled.si()['info'])
    record(f'exit 0 after {seconds} s, WLED released', {'seconds': seconds, 'wled': si['state']})
