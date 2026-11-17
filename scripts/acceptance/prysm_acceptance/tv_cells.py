"""The TV cells: a TV package installed on the owner's LG webOS TV, lighting WLED itself (full) or streaming
to a receiver that does (slim).

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

Its checks are the webOS package's own (registered, reinstall) and, between
them in their order, those any TV runs (`tv_checks`), taken with their plans
for this TV (`taken`). The teardowns undo the set-up in reverse, also after
a failure or Ctrl-C: the cast stopped and the clip server closed, Prysm's app
closed, the receiver stopped and removed, the package uninstalled (capture
stopped, Prysm stopped, then the app removed) with a check that nothing of
Prysm is left, WLED restored. Each fixture and check
says what it does in its docstring and, for the dry run, in `planned`.

Every TV command is a `tv.*` template in `commands.py`; the runner touches
only Prysm's own files there (`commands.TV_READ_PATHS`, `TV_WRITE_PATHS`),
and launches or closes no app but Prysm's.
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
from prysm_acceptance import (
    commands,
    feed,
    host,
    host_cells,
    installed,
    options,
    prysm_api,
    transport,
    tv,
    tv_checks,
    webos,
)
from prysm_acceptance.artifact import Artifact
from prysm_acceptance.bench import Planner, unavailable, uninterrupted, warn_at_end
from prysm_acceptance.checks import Bench, Failed
from prysm_acceptance.config import SSH, Config, Installed
from prysm_acceptance.host import Host, Sender, shown
from prysm_acceptance.host_cells import Clip, Record, wled_as_found, wled_restored, wled_saved
from prysm_acceptance.options import Flavour, TvCell
from prysm_acceptance.prysm_api import Prysm
from prysm_acceptance.tv_checks import CAPTURE, Check, Lit, Screen, Shown, capture, described, in_state
from prysm_acceptance.web import JsonObject
from prysm_acceptance.webos import LunaReply, WebosTv
from prysm_acceptance.wled_api import Wled

CELLS = list(options.TV_CELLS)
planned = Planner[TvCell]()
# The apps a cell may start from: none named, the home screen (also under the screen saver), Prysm's own app,
# and the Cast receiver the clip plays in. Anything else in front: the runner brings the home screen there.
HOME_APP = commands.HOME_APP
FREE_APPS = ('', HOME_APP, commands.APP, webos.CAST_APP)
# For the dry run, where the configuration names no TV.
EXAMPLE_TV = 'root@TV'
# Set once a TV cell of this run starts installing its package: from then on the data files on the TV
# (which no uninstall removes) are the run's own, not the owner's.
INSTALLED_BY_RUN = pytest.StashKey[bool]()


@dataclass(frozen=True)
class OnTv:
    """The TV, and what of Prysm was on it before the run (`WebosTv.found`)."""

    tv: WebosTv
    found: list[str]


def at_home(found: WebosTv) -> str:
    """The app found in front. Over one a cell cannot start from (`FREE_APPS`) the home screen is brought to
    the front; Failed unless the TV reports it there within `tv_checks.CONFIRM_SECONDS`."""
    front = found.foreground()
    if front in FREE_APPS:
        return front
    found.launch_home()
    deadline = time.monotonic() + tv_checks.CONFIRM_SECONDS
    while (now := found.foreground()) != HOME_APP:
        if time.monotonic() >= deadline:
            raise Failed(f'the home screen is not in front: {now or "no app"} is, after {front}')
        time.sleep(1)
    return front


def _ssh(config: Config) -> str:
    return config.tv.ssh or EXAMPLE_TV


def _ipk(cell: TvCell, config: Config) -> Path | None:
    return config.tv.ipk(cell.flavour)


def _settings_document(config: Config, section: JsonObject) -> prysm_api.Settings:
    return host.settings(host.wled_output(config), section)


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
        f'(saved as tv-foreground.json; with another app than {", ".join(app or "none" for app in FREE_APPS)} '
        'in front:)',
        *shown(commands.tv_launch_home(ssh)),
        f'(the home screen not in front within {tv_checks.CONFIRM_SECONDS} s fails the cell before anything else '
        'changes)',
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
        f'then {commands.tv_reachable(_ssh(config)).display()} every 5 s for up to {webos.WAKE_SECONDS} s',
    ]


@pytest.fixture(scope='module')
@planned(does=_target_plan)
def target(cell: TvCell, config: Config, bench: Bench, request: pytest.FixtureRequest, artifact: Artifact) -> OnTv:
    """The TV: reachable over SSH (woken first when `[tv] mac` is set), at a known start, and what of Prysm is
    on it.

    The start: the app in front (saved, and never brought back at the end) is none, the home screen, Prysm's or
    the Cast receiver; over any other the runner brings the home screen (`at_home`).
    With `[tv] installed = "refuse"` anything of Prysm found
    (the app, the service, its manifests or boot hook, a process, the Luna service, the data files an uninstall
    leaves) skips the cell, or fails it when named, before anything changes."""
    found = WebosTv(bench.transport, config.tv.ssh)
    unreachable = found.reach(config.tv.mac, lambda: host.local_address(_tv_host(config)))
    if unreachable:
        unavailable(request, cell, unreachable)
    bench.save('tv-foreground.json', {'foreground': at_home(found)})
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
        'capture stopped first, when Prysm answers:',
        *shown(commands.tv_get_state(ssh)),
        'full: PUT PORTAL/api/v1/target off (PORTAL as getState reports it; skipped when it does not answer)',
        *shown(commands.tv_set_enabled(ssh, False)),
        '(slim only)',
        *shown(commands.tv_supervisor_uninstall(ssh), commands.tv_processes(ssh)),
        f'(every 2 s until no Prysm process is left; one still there after {webos.STOP_SECONDS} s fails, and the '
        'app is NOT removed: removing it would end Prysm abruptly)',
        *shown(
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

    At the end, also after a failure, it is uninstalled (capture stopped, the supervisor's uninstall, no Prysm
    process left, then the app removed through the TV's install service) and nothing of Prysm may be left but
    its data files. While a Prysm process still runs the app is left installed and the teardown fails."""
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


def _planning(config: Config) -> WebosTv:
    """The TV as the dry run shows its commands: nothing runs."""
    return WebosTv(transport.Refusing(), _ssh(config))


def taken(check: Check) -> Check:
    """A check any TV runs (`tv_checks`), with its plan for this TV."""
    does = tv_checks.PLANS[check]
    return planned(does=lambda cell, config: does(_planning(config), cell, config))(check)


@planned(does=lambda cell, config: shown(commands.tv_get_state(_ssh(config)), commands.tv_paths(_ssh(config))))
def test_registered(cell: TvCell, target: OnTv, package: LunaReply, record: Record) -> None:
    """The service is registered: getState answers as this package with its worker; the app, service,
    manifests and boot hook are there."""
    state = tv.wait_state(target.tv.get_state, lambda s: s['worker']['phase'] in ('running', 'stopped', 'retrying'), 20)
    if state is None or state['flavor'] != cell.flavour:
        raise Failed(f'getState does not answer as {cell.flavour}', state)
    present = target.tv.paths()
    missing = [path for path in commands.INSTALLED_PATHS if not present.get(path)]
    if missing:
        raise Failed(f'missing after setup: {missing}', present)
    record(f'registered; worker {state["worker"]["phase"]}; all files present', {'state': state, 'paths': present})


test_video_only = taken(tv_checks.test_video_only)
test_video_with_overlay = taken(tv_checks.test_video_with_overlay)
test_wled_live = taken(tv_checks.test_wled_live)
test_fps = taken(tv_checks.test_fps)
test_capturing = taken(tv_checks.test_capturing)
test_capture_modes = taken(tv_checks.test_capture_modes)
test_offer = taken(tv_checks.test_offer)
test_encodings = taken(tv_checks.test_encodings)
test_pause_resume = taken(tv_checks.test_pause_resume)
test_soak = taken(tv_checks.test_soak)


@planned(
    does=lambda cell, config: [
        *tv_checks.state_plan(_planning(config), Shown.VIDEO, config),
        *_install_plan(_ssh(config), str(_ipk(cell, config) or 'IPK')),
        '(getState until the worker runs, 60 s; full: GET PORTAL/api/v1/state); the video state confirmed again',
    ]
)
def test_reinstall(cell: TvCell, config: Config, screen: Screen, target: OnTv, prysm: Prysm, record: Record) -> None:
    """Reinstall while lighting, in the video state: the same package installed again keeps the preferences (and
    the full package's settings), lighting or the stream resumes by itself, and the TV still shows the video."""
    reached = in_state(screen, Shown.VIDEO, prysm, tv_checks.video_section(cell))
    before = cast('LunaReply', target.tv.get_state())
    settings = prysm.state()['settings']
    target.tv.install(str(_ipk(cell, config)))
    after = tv.wait_state(target.tv.get_state, lambda s: s['worker']['phase'] == 'running', 60)
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


test_ui_only = taken(tv_checks.test_ui_only)
test_ui_off = taken(tv_checks.test_ui_off)
test_standby = taken(tv_checks.test_standby)
