"""The TV cells: a TV package installed on the owner's LG TV, lighting WLED itself (full) or streaming to a
receiver that does (slim).

`acceptance tv` and `acceptance all --tv-is-free` run this module with pytest
and the `bench` plugin; a cell is the `cell` parameter (`tv-full`,
`tv-slim-mac`, `tv-slim-linux`). Its fixtures set up, in order: the package
(and the slim receiver's binary), the TV (reached over SSH, woken with
wake-on-LAN when `[tv] mac` is set, and what of Prysm is on it: refused, or
replaced after the WLED snapshot), the slim receiver's target (the host
cells' seam), WLED's snapshot, a Prysm already on the TV replaced, the
package installed, the Prysm that lights WLED (the TV's own portal, or the
receiver with the TV streaming to it), what WLED should see, and with
`[tv.feed.cast]` the test clip cast to the TV (started by the colour check,
the only check that needs it). The checks follow in this module's order;
`test_wled_live`, `test_fps` and `test_colours` are the host cells' own. The
teardowns undo it in reverse, also after a failure or Ctrl-C: the cast
stopped and the clip server closed, the receiver stopped and removed, the
package uninstalled with a check that nothing of Prysm is left, WLED
restored. Each fixture and check says what it does in its docstring and, for
the dry run, in `planned`.

Every TV command is a `tv.*` template in `commands.py`; the runner touches
only Prysm's own files there (`commands.TV_READ_PATHS`, `TV_WRITE_PATHS`).
"""

from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
import socket
import time
from typing import cast
from urllib.parse import urlsplit
import warnings

import pytest

from prysm_acceptance import artifact as artifacts
from prysm_acceptance import commands, feed, host, host_cells, installed, options, prysm_api, transport, tv
from prysm_acceptance.artifact import Artifact
from prysm_acceptance.bench import Planner, only, unavailable, uninterrupted, warn_at_end
from prysm_acceptance.checks import Answer, Bench, Failed, ask_person
from prysm_acceptance.config import SSH, Config, Installed
from prysm_acceptance.feed import Feed
from prysm_acceptance.host import Host, Sender, shown
from prysm_acceptance.host_cells import Clip, Record, processed, wled_as_found, wled_restored, wled_saved
from prysm_acceptance.options import Flavour, TvCell
from prysm_acceptance.prysm_api import Prysm, Target, near, rates
from prysm_acceptance.tv import Encoding, LunaReply, Tv
from prysm_acceptance.web import JsonObject
from prysm_acceptance.wled_api import Wled

CELLS = list(options.TV_CELLS)
planned = Planner[TvCell]()
# The capture every cell starts with, and the soaks (width, height, fps): the full package captures at each
# size, the slim package streams hardware H.264.
CAPTURE = (640, 360, 30)
SOAKS = {Flavour.FULL: [(640, 360, 30), (1280, 720, 30)], Flavour.SLIM: [(640, 360, 30)]}
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


def _ssh(config: Config) -> str:
    return config.tv.ssh or EXAMPLE_TV


def _ipk(cell: TvCell, config: Config) -> Path | None:
    return config.tv.ipk(cell.flavour)


def _settings_document(config: Config, capture: JsonObject) -> prysm_api.Settings:
    return host.settings(host.wled_output(config), capture)


def capture(
    flavour: Flavour, width: int, height: int, fps: int, encoding: Encoding = Encoding.H264_HARDWARE
) -> JsonObject:
    """The capture section: the TV's own (full), or the receiver's stream from the TV (slim). With the TV's
    menus and screen saver included, so frames come whatever the TV shows (without them there are none while
    no video plays). The clip cast to the TV plays in an app (the Cast receiver), so it needs them too."""
    size: JsonObject = {'width': width, 'height': height, 'fps': fps, 'include_ui': True}
    if flavour == Flavour.FULL:
        return {'kind': 'webos', **size}
    return {'kind': 'remote', **size, 'encoding': encoding}


def ask_for(prysm: Prysm, section: JsonObject) -> None:
    """Save `section` as the capture and light WLED (Screen sync); fails unless both are accepted."""
    reply = prysm.change_settings(lambda settings: settings.__setitem__('capture', section))
    if reply.status != 200:
        raise Failed(f'capture {section}: HTTP {reply.status}', reply.json())
    time.sleep(2)  # the session restarts with the new capture
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
        *(['[tv.feed.cast]: FFmpeg on PATH for the clip, PyChromecast'] if config.tv.feed else []),
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
    controlled = feed.on_tv(config.tv)
    host_cells.tools(request, cell, controlled, clip=controlled is not None)
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
    """The TV: reachable over SSH (woken first when `[tv] mac` is set), and what of Prysm is on it.

    With `[tv] installed = "refuse"` anything found (the app, the service, its manifests or boot hook, a
    process, the Luna service, the data files an uninstall leaves) skips the cell, or fails it when named,
    before anything changes."""
    found = Tv(bench.transport, config.tv.ssh)
    unreachable = found.reach(config.tv.mac, lambda: host.local_address(_tv_host(config)))
    if unreachable:
        unavailable(request, cell, unreachable)
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
            f'PUT PORTAL/api/v1/settings: WLED output, neutral processing, the webos capture {width}x{height}@{fps}',
        ]
    seam = _receiver_seam(cell, config, transport.Refusing(), Path('RUN_DIR/CELL')).plan(None, '')
    ssh = _ssh(config)
    return [
        f'write RUN_DIR/CELL/headless-settings.json: WLED output, neutral processing, the remote capture '
        f'{width}x{height}@{fps} hardware H.264',
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
    at 30 fps).

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


def _cast(config: Config) -> Feed:
    """The TV's feed; without `[tv.feed.cast]` what needs it is skipped."""
    controlled = feed.on_tv(config.tv)
    if controlled is None:
        pytest.skip('no [tv.feed.cast]: the TV shows whatever it shows, so its colours are not known')
    return controlled


@pytest.fixture(scope='module')
@planned(
    does=lambda cell, config: (
        [f'$ {host_cells.clip_command().display()}'] if config.tv.feed else ['none: no [tv.feed.cast]']
    )
)
def clip(config: Config, bench: Bench, prysm: Prysm) -> Clip:
    """With `[tv.feed.cast]`, the host cells' test clip: generated, its edge colours stepping round the corners
    every 2 s, for 30 minutes (longer than the cell, so it never ends or loops)."""
    _cast(config)
    return host_cells.generated(bench)


@pytest.fixture(scope='module')
@planned(
    does=lambda cell, config: (
        host_cells.feed_plan(feed.on_tv(config.tv)) or ['none: the TV shows whatever it shows; test_colours skipped']
    ),
    undoes=lambda cell, config: host_cells.feed_undone(feed.on_tv(config.tv)),
)
def feed_shown(config: Config, bench: Bench, prysm: Prysm, clip: Clip) -> Iterator[Feed]:
    """With `[tv.feed.cast]`, the clip cast to the TV's own Cast receiver; stopped at the end.

    Started by test_colours, the only check that needs it, once the Prysm that lights WLED runs with its
    settings; it plays on through the later checks. The TV's first cast from this computer waits for a
    person to accept it on the TV: without PLAYING in time the runner asks on this terminal, and a skip (or no
    terminal) skips only test_colours. A cast that fails fails only test_colours. Slim: the receiver's Prysm
    is the one sampled."""
    with host_cells.showing(_cast(config), bench, clip) as controlled:
        yield controlled


# ---- the checks, in order ----------------------------------------------------


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


# The host cells' own: Screen sync lights WLED from the expected sender, the frame rate holds, and the colours
# follow the clip cast to the TV (skipped without [tv.feed.cast] or when nobody accepted the cast).
test_wled_live = host_cells.test_wled_live
test_fps = host_cells.test_fps
test_colours = host_cells.test_colours


@planned(does=lambda cell, config: ['PUT /api/v1/target processed', *shown(commands.tv_get_state(_ssh(config)))])
def test_capturing(target: OnTv, prysm: Prysm, record: Record) -> None:
    """The TV says it captures: getState capturing with a frame rate while Screen sync runs."""
    processed(prysm)
    state = target.tv.wait_state(lambda s: s['capturing'] and (s.get('fps') or 0) > 0, 10)
    if state is None:
        raise Failed('getState does not say capturing', target.tv.get_state())
    record(f'capturing at {state.get("fps")} fps', state)


@only(Flavour.FULL)
@planned(does=lambda cell, config: ['POST /api/v1/capture/modes {"kind":"webos"}'])
def test_capture_modes(prysm: Prysm, record: Record) -> None:
    """The full package: the TV's capture modes are known."""
    reply = prysm.modes({'kind': 'webos'})
    answer = prysm_api.modes_answer(reply)
    if reply.status != 200 or answer.get('status') != 'known':
        raise Failed(f'capture modes: HTTP {reply.status}', answer)
    record('; '.join(prysm_api.describe_modes(answer['modes'])), answer)


@only(Flavour.SLIM)
@planned(
    does=lambda cell, config: [
        'PUT /api/v1/target off',
        'POST /api/v1/capture/modes {"kind":"remote","encoding":E} for each encoding until known (30 s)',
    ]
)
def test_offer(prysm: Prysm, record: Record) -> None:
    """The slim package: with the lights off, the TV's offer is known for each encoding."""
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
    unknown = [str(encoding) for encoding, answer in found.items() if answer.get('status') != 'known']
    if unknown:
        raise Failed(f'no offer known with lights off for {unknown}', found)
    if prysm.state()['target'] != Target.OFF:
        raise Failed('the receiver is not off', found)
    record('; '.join(f'{encoding}: {len(found[encoding]["modes"])} modes' for encoding in Encoding), found)


@only(Flavour.SLIM)
@planned(
    does=lambda cell, config: [
        f'for each encoding: PUT /api/v1/settings remote capture {CAPTURE[0]}x{CAPTURE[1]}@{CAPTURE[2]}, PUT '
        f'/api/v1/target processed, GET /api/v1/state every second for {config.checks.seconds} s',
        *shown(commands.tv_sample_worker(_ssh(config)), commands.tv_get_state(_ssh(config))),
        'PUT /api/v1/target off',
    ]
)
def test_encodings(config: Config, target: OnTv, prysm: Prysm, record: Record) -> None:
    """The slim package: each encoding streams at 30 fps, and getState names it; the TV worker's CPU.

    Raw has no rate control: its frame rate is what the link to the receiver carries. So the evidence also
    holds, per encoding, the size of the frames that arrived (the TV's UI capture delivers at most 320x180,
    whatever was asked) and the TV's power and video state, to tell a slow link from a larger frame."""
    results: dict[str, object] = {}
    failed: list[str] = []
    summary: list[str] = []
    width, height, fps = CAPTURE
    for encoding in Encoding:
        try:
            ask_for(prysm, capture(Flavour.SLIM, width, height, fps, encoding))
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
    record(
        ', '.join(summary),
        results,
    )


@only(Flavour.SLIM)
@planned(
    does=lambda cell, config: [
        'PUT /api/v1/target processed, then off',
        *shown(commands.tv_get_state(_ssh(config))),
        'PUT /api/v1/target processed',
    ]
)
def test_pause_resume(target: OnTv, prysm: Prysm, record: Record) -> None:
    """The slim package: the receiver's lights off pause the TV's capture; on again resumes it."""
    processed(prysm)
    prysm.set_target(Target.OFF)
    start = time.monotonic()
    paused = target.tv.wait_state(lambda s: s['paused'] and not s['capturing'], 15)
    paused_after = round(time.monotonic() - start, 1)
    prysm.set_target(Target.PROCESSED)
    start = time.monotonic()
    resumed = target.tv.wait_state(lambda s: s['capturing'] and not s['paused'], 20)
    evidence: dict[str, object] = {
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
        f'for {sizes}: PUT /api/v1/settings capture, PUT /api/v1/target processed, then {config.tv.soak_seconds} s '
        'of GET /api/v1/state every second and, every 5 s:',
        *shown(commands.tv_sample_worker(_ssh(config))),
        f'PUT /api/v1/settings capture {CAPTURE[0]}x{CAPTURE[1]}@{CAPTURE[2]} again',
    ]


@planned(does=_soak_plan)
def test_soak(cell: TvCell, config: Config, target: OnTv, prysm: Prysm, record: Record) -> None:
    """The soak: lighting for `[tv] soak_seconds` at each size (full: 640x360 and 1280x720; slim: 640x360,
    hardware H.264) with the frame rate within the tolerance; the TV worker's CPU and RSS from /proc."""
    results: list[tv.Soak] = []
    try:
        for width, height, fps in SOAKS[cell.flavour]:
            ask_for(prysm, capture(cell.flavour, width, height, fps))
            result = target.tv.soak(prysm, config.tv.soak_seconds)
            result['size'] = f'{width}x{height}@{fps}'
            results.append(result)
            if not near(result['fps_mean'], fps, config.checks.fps_tolerance):
                raise Failed(f'{result["fps_mean"]} fps over the soak at {result["size"]}, expected {fps}', results)
    finally:
        ask_for(prysm, capture(cell.flavour, *CAPTURE))
    record('; '.join(f'{r["size"]}: {tv.soak_summary(r)}' for r in results), results)


@planned(
    does=lambda cell, config: [
        'PUT /api/v1/target processed; a person puts the TV in standby with the remote, then wakes it',
        *shown(commands.tv_get_state(_ssh(config))),
        '(every 3 s: standby seen if the TV answers, then active, worker running and capturing within 90 s)',
    ]
)
def test_standby(target: OnTv, prysm: Prysm, record: Record) -> None:
    """Standby and wake (a person presses power): getState shows standby if the TV answers, then active with
    the worker running and capturing within 90 s of waking. The runner never puts the TV to sleep."""
    processed(prysm)
    answer, note = ask_person(
        "Put the TV in standby with the remote's power button (do not unplug). Answer y once the screen is off.",
        (Answer.YES, Answer.SKIP),
    )
    if answer != Answer.YES:
        pytest.skip(f'standby skipped {note}'.strip())
    asleep = target.tv.wait_state(lambda s: s['power'] != 'active', 60, every=3)
    evidence: dict[str, object] = {'standby_state': asleep}
    answer, note = ask_person(
        'Wake the TV with the power button and start video. Answer y when the picture is back.',
        (Answer.YES, Answer.SKIP),
    )
    if answer != Answer.YES:
        pytest.skip(f'wake skipped {note}'.strip())
    awake = target.tv.wait_state(
        lambda s: s['power'] == 'active' and s['worker']['phase'] == 'running' and s['capturing'], 90, every=3
    )
    evidence['wake_state'] = awake
    if awake is None:
        raise Failed('capture did not resume within 90 s of waking', evidence)
    record(f'standby seen: {asleep["power"] if asleep else "no answer"}; capture resumed', evidence)


@planned(
    does=lambda cell, config: [
        'PUT /api/v1/target processed',
        *shown(commands.tv_get_state(_ssh(config))),
        *_install_plan(_ssh(config), str(_ipk(cell, config) or 'IPK')),
        '(getState until the worker runs, 60 s; full: GET PORTAL/api/v1/state)',
    ]
)
def test_reinstall(cell: TvCell, config: Config, target: OnTv, prysm: Prysm, record: Record) -> None:
    """Reinstall while lighting: the same package installed again keeps the preferences (and the full
    package's settings), and lighting or the stream resumes by itself."""
    processed(prysm)
    before = cast('LunaReply', target.tv.get_state())
    settings = prysm.state()['settings']
    target.tv.install(str(_ipk(cell, config)))
    after = target.tv.wait_state(lambda s: s['worker']['phase'] == 'running', 60)
    evidence: dict[str, object] = {'before': before, 'after': after}
    if after is None:
        raise Failed('the worker is not running 60 s after the reinstall and setup', evidence)
    if after['preferences'] != before['preferences']:
        raise Failed('preferences changed by the reinstall', evidence)
    if cell.flavour == Flavour.FULL and target.tv.wait_portal().state()['settings'] != settings:
        raise Failed('settings.json changed by the reinstall', evidence)
    if prysm.wait(prysm_api.running, 30) is None:
        raise Failed('lighting did not resume after the reinstall', evidence)
    record('preferences (and settings) kept, lighting resumed', evidence)
