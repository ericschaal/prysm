"""The TV cells: a TV package installed on the owner's LG webOS TV, lighting WLED itself (full) or streaming
to a receiver that does (slim).

`acceptance tv` and `acceptance all --tv-is-free` run this module with pytest
and the `bench` plugin; a cell is the `cell` parameter (`tv-full`,
`tv-slim-mac`, `tv-slim-linux`). Its fixtures set up, in order: the package
(and the slim receiver's binary), the TV (reached over SSH, woken with
wake-on-LAN when `[tv] mac` is set, and what of Prysm is on it: refused, or
replaced before the WLED snapshot; nobody using it: only the home screen,
Prysm or the Cast receiver in front), the slim receiver's target (the host
cells' seam), a Prysm already on the TV replaced (its own lighting of WLED
stopped, any other sender refused), WLED's snapshot, the package installed,
the Prysm that lights WLED (the TV's own portal, or the receiver with the TV
streaming to it), what WLED should see, the test clip, and the `screen`:
what the TV shows.

Its checks are the webOS package's own (registered, reinstall) and, between
them in their order, those any TV runs (`tv_checks`), taken with their plans
for this TV (`taken`). The teardowns undo the set-up in reverse, also after
a failure or Ctrl-C: the cast stopped and the clip server closed, Prysm's app
closed, the receiver stopped and removed, the package uninstalled with a
check that nothing of Prysm is left, WLED restored. Each fixture and check
says what it does in its docstring and, for the dry run, in `planned`.

Every TV command is a `tv.*` template in `commands.py`; the runner touches
only Prysm's own files there (`commands.TV_READ_PATHS`, `TV_WRITE_PATHS`),
and launches or closes no app but Prysm's.
"""

from collections.abc import Callable, Generator, Iterator
from contextlib import AbstractContextManager, contextmanager
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
from prysm_acceptance.bench import Planner, only, unavailable, uninterrupted, warn_at_end
from prysm_acceptance.checks import Answer, Bench, Failed, ask_person
from prysm_acceptance.config import SSH, Config, Installed
from prysm_acceptance.host import Host, Sender, shown
from prysm_acceptance.host_cells import Clip, Record, wled_as_found, wled_restored, wled_saved
from prysm_acceptance.options import Flavour, TvCell
from prysm_acceptance.prysm_api import Prysm
from prysm_acceptance.tv_checks import (
    CAPTURE,
    Check,
    Lit,
    Screen,
    Shown,
    capture,
    in_state,
    receiver_seam,
)
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
# How long WLED may stay live after the TV's Prysm was uninstalled, as after a host's SIGTERM (`test_sigterm`):
# the worker has stopped by then, and WLED leaves live a few seconds after the last frame.
RELEASED_SECONDS = 10


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


def _prysm_address(seam: Host, tv_host: str) -> str:
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
    warning = artifacts.stale(found, bench.transport)
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
        *(
            [
                *shown(commands.device_reader_tools(ssh)),
                '(python3 or timeout missing, for the colour checks read on the TV: skips the cell, or fails it '
                'when named)',
            ]
            if cell.flavour == Flavour.FULL
            else []
        ),
        *shown(commands.tv_foreground(ssh)),
        f'(saved as tv-foreground.json; with another app than {", ".join(app or "none" for app in FREE_APPS)} '
        'in front:)',
        *shown(commands.tv_launch_home(ssh)),
        f'(the home screen not in front within {tv_checks.CONFIRM_SECONDS} s fails the cell before anything else '
        'changes)',
        *shown(commands.tv_processes(ssh)),
        f'(the old {commands.CAPTURE_APP} app running fails the cell)',
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
            else 'backed up, then uninstalled, before the WLED snapshot; [tv] installed = "replace")'
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
    if cell.flavour == Flavour.FULL:  # the full package's colours are read on the TV; the slim one's on the receiver
        missing = host.reader_missing(bench.transport, found.target)
        if missing:
            unavailable(request, cell, missing)
    bench.save('tv-foreground.json', {'foreground': at_home(found)})
    old = [line for line in found.processes() if commands.CAPTURE_APP in line[-1]]
    if old:
        pytest.fail(f'the old {commands.CAPTURE_APP} app is running; remove it first (docs/users/webos.md): {old}')
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
    return receiver_seam(cell, config, transport.Refusing(), Path('RUN_DIR/CELL')).plan(None, '').preflight


@pytest.fixture(scope='module')
@planned(does=_receiver_plan)
def receiver(cell: TvCell, config: Config, bench: Bench, request: pytest.FixtureRequest, target: OnTv) -> Host | None:
    """Slim: where the receiver runs (the host cells' seam on `[mac]` or `[linux]`), and whether it can.

    Over SSH: reachable without a prompt, Linux on the binary's machine, the listen port free."""
    if not cell.receiver:
        return None
    seam = receiver_seam(cell, config, bench.transport, bench.out_dir)
    try:
        reason = seam.preflight(None, '')
    except transport.CommandFailed as error:
        reason = f'the receiver preflight failed: {error}'
    if reason:
        unavailable(request, cell, reason)
    return seam


def clear(found: WebosTv, prysm: list[str], lights: Wled, path: Path, warn: Callable[[str], None]) -> str:
    """`installed = "replace"`: the Prysm the TV had backed up and uninstalled (`installed.replace`).

    While WLED is live only the TV's own sender (the host of the portal address getState reports, as
    `running` takes it; a slim Prysm's receiver is not the TV) goes on; any other stops the cell before
    anything changes. The TV's own Prysm is
    backed up still lighting, so the backup records its lighting as found, and the uninstall stops it; WLED
    must then leave live within `RELEASED_SECONDS`, so the WLED snapshot after it is of WLED undriven, as for
    every cell."""
    sender = lights.si()['info']
    if sender.get('live'):
        portal = tv.portal(found.get_state() or LunaReply())
        if not portal or urlsplit(portal).hostname != sender.get('lip'):
            raise Failed(host_cells.driven(sender.get('lip')))
    done = installed.replace(found, prysm, path, warn)
    if sender.get('live') and lights.wait(lambda si: not si['info'].get('live'), RELEASED_SECONDS) is None:
        raise Failed(f'WLED is still live {RELEASED_SECONDS} s after Prysm on the TV was uninstalled', lights.si())
    return done


def _cleared_plan(cell: TvCell, config: Config) -> list[str]:
    ssh = _ssh(config)
    return host_cells.replaced(
        'tv',
        config.tv.installed,
        [
            f'GET http://{config.wled}/json/si; while WLED is live:',
            *shown(commands.tv_get_state(ssh)),
            '(a sender that is not the TV, the host of the portal address getState reports, refuses the cell before '
            'anything changes)',
            *shown(*(commands.tv_read_data(ssh, name) for name in commands.DATA_FILES), commands.tv_get_state(ssh)),
            '(nothing answering while Prysm is installed stops the cell, nothing removed)',
            'the full package:',
            *shown(commands.tv_get_state(ssh)),
            'GET PORTAL/api/v1/state (PORTAL as getState reports it, within 40 s)',
            'then, unless only data files were found:',
            *_uninstall_plan(config),
            f'when the TV was lighting WLED: GET http://{config.wled}/json/si until it is not live (within '
            f'{RELEASED_SECONDS} s, else the cell fails), before the WLED snapshot',
        ],
    )


@pytest.fixture(scope='module')
@planned(does=_cleared_plan)
def cleared(config: Config, bench: Bench, request: pytest.FixtureRequest, target: OnTv, receiver: Host | None) -> None:
    """With `[tv] installed = "replace"`, the Prysm the TV had backed up and uninstalled (`clear`): the TV's own
    lighting of WLED is stopped by the uninstall, after the backup recorded it; another sender is refused before
    anything is removed.

    Before the WLED snapshot, so the snapshot is of WLED undriven. Its data files stay on the TV (the runner
    never deletes them); the run changes them, and the backup holds them as they were."""
    if not target.found:
        return
    path = bench.out_dir / installed.BACKUP
    bench.log(clear(target.tv, target.found, Wled(config.wled), path, lambda text: warn_at_end(request, text)))


@pytest.fixture(scope='module')
@planned(
    does=lambda cell, config: [
        *wled_saved(cell, config),
        '(after cleared: a Prysm replaced on the TV, and its own lighting, are gone by then)',
    ],
    undoes=wled_restored,
)
def wled(config: Config, bench: Bench, cleared: None) -> Iterator[Wled]:
    """WLED as found, saved before anything changes but the replaced Prysm (`cleared`) and put back last, also
    after a failure or Ctrl-C."""
    with wled_as_found(config, bench) as client:
        yield client


def _stop_plan(ssh: str) -> list[str]:
    return [
        *shown(commands.tv_uninstall(ssh), commands.tv_processes(ssh)),
        f'(every 2 s until no Prysm process is left; one still there after {webos.STOP_SECONDS} s fails, and '
        'Prysm is left installed as it is: removing or replacing it would end it abruptly)',
    ]


def _install_plan(ssh: str, ipk: str, before_setup: tuple[str, ...] = ()) -> list[str]:
    return [
        *shown(commands.verify_ipk(ipk), commands.tv_stage(ssh, ipk)),
        *_stop_plan(ssh),
        *shown(commands.tv_install(ssh), commands.tv_unstage(ssh)),
        *before_setup,
        *shown(commands.tv_setup(ssh), commands.tv_get_state(ssh)),
    ]


def _uninstall_plan(config: Config) -> list[str]:
    ssh = _ssh(config)
    return [
        *_stop_plan(ssh),
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
    cell: TvCell, config: Config, request: pytest.FixtureRequest, target: OnTv, wled: Wled
) -> Iterator[LunaReply]:
    """The package installed the documented way (verified, staged, Prysm stopped, appInstallService, unstaged,
    setup.sh).

    At the end, also after a failure, it is uninstalled (Prysm's Luna uninstall, no Prysm process left, the
    supervisor's uninstall, then the app removed through the TV's install service) and nothing of Prysm may be
    left but its data files. While a Prysm process still runs the app is left installed and the teardown fails."""
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
    seam = receiver_seam(cell, config, transport.Refusing(), Path('RUN_DIR/CELL')).plan(None, '')
    ssh = _ssh(config)
    return [
        f'write RUN_DIR/CELL/headless-settings.json: WLED output, neutral processing, the remote capture '
        f'{width}x{height}@{fps} hardware H.264 without the UI',
        *seam.start,
        *shown(
            commands.tv_set_preferences(ssh, False, 'http://192.0.2.20:18787'),
            commands.tv_set_enabled(ssh, True),
        ),
        '(prysm_address: the receiver as the TV reaches it, shown as http://192.0.2.20:18787)',
    ]


def _prysm_undo(cell: TvCell, config: Config) -> list[str]:
    if cell.flavour == Flavour.FULL:
        return []
    return receiver_seam(cell, config, transport.Refusing(), Path('RUN_DIR/CELL')).plan(None, '').stop


@pytest.fixture(scope='module')
@planned(does=_prysm_plan, undoes=_prysm_undo)
def prysm(
    cell: TvCell, config: Config, bench: Bench, target: OnTv, receiver: Host | None, package: LunaReply
) -> Iterator[Prysm]:
    """The Prysm that lights WLED, with this run's settings (WLED output, neutral processing, a 640x360 capture
    at 30 fps without the UI; each check asks for its own).

    Full: the TV's own portal, at the address getState reports. Slim: the receiver started with the remote
    capture, and the TV pointed at it (`prysm_address`) and enabled; the receiver is stopped and removed at the end."""
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
        address = _prysm_address(receiver, _tv_host(config))
        target.tv.set_preferences({'autostart': False, 'prysm_address': address})
        target.tv.call(commands.tv_set_enabled(target.tv.target, True))
        bench.log(f'The TV streams to {address}')
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
        portal = urlsplit(prysm.base)
        return Lit(target.tv, portal.hostname or '', bench.out_dir, port=portal.port or 80)
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
    state = tv.wait_state(target.tv.get_state, lambda s: s['worker']['phase'] in ('running', 'stopped', 'asking'), 20)
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


def _reinstall_plan(cell: TvCell, config: Config, *, on: bool) -> list[str]:
    before_setup = ()
    expected = ['GET receiver /api/v1/state until frames resume']
    if cell.flavour == Flavour.FULL:
        before_setup = (tv_checks.idle_lights_plan(config, on=on),)
        expected = [
            'GET PORTAL/api/v1/state: saved settings and remembered Screen sync',
            'frames and live DDP from the TV' if on else f'Off and no live stream for {tv_checks.QUIET_SECONDS} s',
            'GET WLED /json/si to verify the physical output',
        ]
    return [
        *tv_checks.state_plan(_planning(config), Shown.VIDEO, config),
        *_install_plan(_ssh(config), str(_ipk(cell, config) or 'IPK'), before_setup),
        'getState until the worker runs (60 s); verify preferences and the video on screen',
        *expected,
    ]


def _reinstall(
    cell: TvCell, config: Config, screen: Screen, target: OnTv, prysm: Prysm, wled: Wled, *, on: bool
) -> tuple[Prysm, dict[str, object]]:
    reached = in_state(screen, Shown.VIDEO, prysm, tv_checks.video_section(cell))
    before = target.tv.get_state()
    if before is None:
        raise Failed('the supervisor did not answer before reinstall')
    settings = prysm.state()['settings']
    evidence: dict[str, object] = {'state': reached, 'before': before}
    target.tv.install_files(str(_ipk(cell, config)))
    if cell.flavour == Flavour.FULL:
        evidence['wled_before_setup'] = tv_checks.idle_lights(wled, on=on)
    target.tv.start_service()
    after = tv.wait_state(target.tv.get_state, lambda s: s['worker']['phase'] == 'running', 60)
    evidence['after'] = after
    if after is None or after['preferences'] != before['preferences']:
        raise Failed('expected a running worker with its original preferences after reinstall', evidence)
    if cell.flavour == Flavour.FULL:
        prysm = target.tv.wait_portal()
        if prysm.state()['settings'] != settings:
            raise Failed('settings changed across reinstall', evidence)
    evidence['state_after'] = screen.confirm(Shown.VIDEO)
    return prysm, evidence


@planned(does=lambda cell, config: _reinstall_plan(cell, config, on=False))
def test_reinstall(
    cell: TvCell, config: Config, screen: Screen, target: OnTv, prysm: Prysm, wled: Wled, record: Record
) -> None:
    """Reinstall during Screen sync. Full remembers Screen sync and leaves WLED off. Slim resumes streaming."""
    portal, evidence = _reinstall(cell, config, screen, target, prysm, wled, on=False)
    if cell.flavour == Flavour.FULL:
        tv_checks.check_startup_off(portal, wled, evidence)
        record('preferences retained and lights stayed off', evidence)
    else:
        if portal.wait(prysm_api.running, 30) is None:
            raise Failed('the slim receiver did not resume with frames', evidence)
        record('preferences retained and stream resumed', evidence)


@only(Flavour.FULL)
@planned(does=lambda cell, config: _reinstall_plan(cell, config, on=True))
def test_reinstall_wled_on(
    cell: TvCell,
    config: Config,
    screen: Screen,
    target: OnTv,
    prysm: Prysm,
    wled: Wled,
    running: Sender,
    record: Record,
) -> None:
    """Full resumes remembered Screen sync when WLED is powered on and idle before startup."""
    portal, evidence = _reinstall(cell, config, screen, target, prysm, wled, on=True)
    state = portal.wait(prysm_api.running, 30)
    evidence['portal_after'] = state or portal.state()
    if (
        state is None
        or state['target'] != prysm_api.Target.SCREEN_SYNC
        or state['control']['remembered'] != 'screen_sync'
    ):
        raise Failed('expected remembered Screen sync to resume with frames', evidence)
    lights = host_cells.live_from_host(portal, wled, running)
    evidence['wled_after'] = lights
    if not lights['state']['on']:
        raise Failed('WLED received DDP but its power is off', evidence)
    record('Screen sync resumed automatically and WLED received live DDP from the TV', evidence)


test_ui_only = taken(tv_checks.test_ui_only)
test_ui_off = taken(tv_checks.test_ui_off)


@contextmanager
def remote_standby() -> Generator[None]:
    def press(prompt: str) -> None:
        answer, note = ask_person(prompt, (Answer.YES, Answer.SKIP))
        if answer != Answer.YES:
            pytest.skip(f'standby/wake skipped {note}'.strip())

    press("Put the TV in standby with the remote's power button. Answer y when the screen is off.")
    try:
        yield
    finally:
        press('Wake the TV with the power button. Answer y when the picture is back.')


@pytest.fixture
def standby(config: Config, target: OnTv) -> AbstractContextManager[None]:
    if not config.tv.mac:
        return remote_standby()
    wake = commands.wake_on_lan(config.tv.mac, host.local_address(config.tv.host))
    return target.tv.standby(wake)


def _standby_plan(cell: TvCell, config: Config) -> list[str]:
    sleep, wake = 'a person puts the TV in standby with the remote', 'a person wakes the TV'
    if config.tv.mac:
        sleep = commands.tv_standby(_ssh(config)).display()
        wake = commands.wake_on_lan(config.tv.mac, '192.0.2.10').display()
    return tv_checks.standby_plan(_planning(config), cell, config, sleep, wake)


test_standby = planned(does=_standby_plan)(tv_checks.test_standby)


def _reboot_plan(config: Config, during_outage: str) -> list[str]:
    ssh = _ssh(config)
    return [
        *shown(commands.tv_sample_worker(ssh), commands.tv_reboot(ssh), commands.tv_reachable(ssh)),
        'wait for SSH to disconnect',
        during_outage,
        'wait for SSH to return, confirm uptime reset and wait for the supervisor before querying Prysm',
        *shown(commands.tv_sample_worker(ssh), commands.tv_processes(ssh), commands.tv_get_state(ssh)),
    ]


def _slim_reboot_plan(config: Config, autostart: bool) -> list[str]:
    ssh = _ssh(config)
    address = 'http://192.0.2.10:18787'
    return [
        'Configure the receiver with remote UI capture and null output; request Screen sync and verify frames',
        *shown(commands.tv_set_preferences(ssh, autostart, address)),
        *shown(commands.tv_set_enabled(ssh, autostart), commands.tv_set_enabled(ssh, not autostart)),
        *shown(
            commands.tv_uninstall(ssh), commands.tv_processes(ssh), commands.tv_setup(ssh), commands.tv_get_state(ssh)
        ),
        'verify manual enablement survives service restart and check received frames in GET /metrics',
        *_reboot_plan(config, 'record receiver frame count during the SSH outage'),
        f'verify enabled={autostart} after reboot and check received frames independently on the receiver',
        'restore slim preferences and enablement',
    ]


def _check_slim_stream(
    found: WebosTv, portal: Prysm, *, enabled: bool, autostart: bool, baseline: int
) -> dict[str, object]:
    """Require new frames while sending, or a stopped worker and an unchanged count for 30 seconds."""
    state = tv.wait_state(found.get_state, lambda _: True, 30)
    deadline = time.monotonic() + (45 if enabled else 30)
    while True:
        frames = portal.frames_received()
        evidence: dict[str, object] = {'tv': state, 'frames_before': baseline, 'frames_after': frames}
        if (
            state is None
            or state.get('enabled') is not enabled
            or state['preferences'].get('autostart') is not autostart
        ):
            raise Failed('slim enablement or Start with TV differs from the request', evidence)
        if enabled:
            if tv.capturing(state) and tv.encoding(state) is not None and frames > baseline:
                return evidence
        elif state['worker']['phase'] != 'stopped' or frames != baseline:
            raise Failed('disabled slim streamed or its worker did not stay stopped', evidence)
        if time.monotonic() >= deadline:
            if enabled:
                raise Failed('enabled slim did not deliver new frames', evidence)
            return evidence
        time.sleep(1)
        state = found.get_state()


def _slim_reboot(found: WebosTv, portal: Prysm, *, autostart: bool) -> dict[str, object]:
    settings = host.settings({'kind': 'null'}, capture(Flavour.SLIM, 320, 180, 15, True, tv_checks.Encoding.RAW))
    if portal.replace_settings(settings).status != 200:
        raise Failed('receiver refused the reboot test settings')
    state = found.get_state()
    if state is None:
        raise Failed('slim did not answer before the reboot test')
    preferences = state['preferences']
    try:
        found.set_preferences({**preferences, 'autostart': autostart})
        if not autostart:
            found.call(commands.tv_set_enabled(found.target, False))
            if tv.wait_state(found.get_state, lambda s: s['worker']['phase'] == 'stopped', 30) is None:
                raise Failed('could not disable slim before the manual enable request')
        found.call(commands.tv_set_enabled(found.target, True))
        host_cells.screen_sync(portal, 45)
        initial = _check_slim_stream(
            found, portal, enabled=True, autostart=autostart, baseline=portal.frames_received()
        )
        if autostart:
            found.call(commands.tv_set_enabled(found.target, False))
            if (
                tv.wait_state(found.get_state, lambda s: not s['enabled'] and s['worker']['phase'] == 'stopped', 30)
                is None
            ):
                raise Failed('slim did not stop after manual disable')
        before = found.sample_worker()['uptime']
        found.stop()
        baseline = portal.frames_received()
        found.start_service()
        restarted = _check_slim_stream(found, portal, enabled=not autostart, autostart=autostart, baseline=baseline)
        if found.sample_worker()['uptime'] < before:
            raise Failed('TV rebooted during the service-only restart')
        with found.reboot() as boot:
            offline = portal.frames_received()
        after = _check_slim_stream(
            found, portal, enabled=autostart, autostart=autostart, baseline=portal.frames_received()
        )
        return {
            'initial': initial,
            'service_restart': restarted,
            'reboot': boot,
            'frames_while_offline': offline,
            'after_reboot': after,
        }
    finally:
        found.set_preferences(preferences)
        found.call(commands.tv_set_enabled(found.target, state['enabled']))


@only(Flavour.SLIM)
@planned(does=lambda cell, config: _slim_reboot_plan(config, True))
def test_reboot_autostart_on(target: OnTv, prysm: Prysm, record: Record) -> None:
    """Slim stays manually disabled through a service restart and streams on the next boot."""
    record('disabled after service restart, streaming after reboot', _slim_reboot(target.tv, prysm, autostart=True))


@only(Flavour.SLIM)
@planned(does=lambda cell, config: _slim_reboot_plan(config, False))
def test_reboot_autostart_off(target: OnTv, prysm: Prysm, record: Record) -> None:
    """Slim resumes manual streaming after a service restart and stays disabled after reboot."""
    record('streaming after service restart, disabled after reboot', _slim_reboot(target.tv, prysm, autostart=False))


@only(Flavour.FULL)
@planned(
    does=lambda cell, config: [
        'Remember Screen sync, set Prysm Off and confirm the session stopped',
        *_reboot_plan(config, 'POST /json/state on=false, lor=2, tt=0 and verify idle WLED'),
        'verify the worker and portal return, Effects remembered, lights off and override retained',
    ]
)
def test_reboot_wled_override_off(target: OnTv, prysm: Prysm, wled: Wled, record: Record) -> None:
    """Full starts its worker and leaves WLED off with Effects remembered under its persistent override."""
    if (
        prysm.set_target(prysm_api.Target.SCREEN_SYNC).status != 204
        or prysm.wait(lambda s: s['control']['remembered'] == 'screen_sync' and not s['control']['busy'], 30) is None
    ):
        raise Failed('could not establish remembered Screen sync before reboot')
    if prysm.set_target(prysm_api.Target.OFF).status != 204:
        raise Failed('Prysm refused Off before reboot')
    if (
        prysm.wait(lambda s: s['target'] == prysm_api.Target.OFF and s['control']['remembered'] == 'screen_sync', 30)
        is None
    ):
        raise Failed('expected Off with Screen sync remembered before reboot')
    tv_checks.idle_lights(wled, on=False)

    def off_override(si: tv_checks.Si) -> bool:
        return not si['state']['on'] and si['state']['lor'] == 2 and not si['info']['live']

    with target.tv.reboot() as boot:
        wled.post_state({'on': False, 'lor': 2, 'tt': 0})
        if wled.wait(off_override, 10) is None:
            raise Failed('could not establish WLED off with persistent override during reboot', wled.si())
    seen = tv.wait_state(target.tv.get_state, lambda s: tv.running(s) is not None, 60)
    if seen is None:
        raise Failed('full worker did not start after reboot')
    evidence: dict[str, object] = {'reboot': boot, 'supervisor': seen}
    tv_checks.check_startup_off(target.tv.wait_portal(), wled, evidence, remembered='effects')
    if not off_override(wled.si()):
        raise Failed('WLED did not retain its override', evidence)
    record('running worker, Effects remembered, lights off and override retained', evidence)
