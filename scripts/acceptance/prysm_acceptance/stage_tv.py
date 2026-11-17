"""Stage B: the full and slim TV packages on a rooted LG TV (run only when the owner says the TV is free).

Every TV command comes from `commands`. WLED and the TV's state are saved
first; at the end the package found on the TV is installed again with its
preferences, settings and lighting (`tv.restore_flavour`), and WLED is put
back last, after anything that drives it has stopped.
"""

from collections.abc import Callable, Iterable, Sequence
from pathlib import Path
import time
from typing import assert_never, cast

from prysm_acceptance import artifact, commands, prysm_api, tv, tv_full, tv_slim, web, wled_api
from prysm_acceptance.checks import Answer, Failed, Skipped
from prysm_acceptance.commands import Command, Flavour
from prysm_acceptance.config import Config, RestoreFlavour
from prysm_acceptance.options import Options
from prysm_acceptance.plan import Bench, Context, Mode, Step
from prysm_acceptance.prysm_api import Target
from prysm_acceptance.stage_data import AsFound, TvData

Ctx = Context[TvData]
Run = Callable[[Ctx], object]

CLEANUPS = (
    "the local receiver stopped (or a configured receiver's settings and target put back)",
    'the TV: the package found at the start installed again (tv.restore_flavour) with setup.sh, its '
    'preferences set back, full: its portal settings and lighting target, slim: enabled',
    'WLED restored from wled-snapshot.json and compared',
)
EXAMPLE_RECEIVER = 'http://192.0.2.20:8788'  # a documentation address, for the dry run


def refuse(config: Config, args: Options) -> str | None:
    if not args.tv_is_free:
        return 'tv: pass --tv-is-free once the owner has said the TV may be used now (or use --dry-run)'
    if not config.tv.ssh or not config.tv.address:
        return 'tv: tv.address is not set'
    if not config.wled:
        return 'tv: wled.address is needed (WLED is snapshotted and restored)'
    if Flavour.SLIM in config.tv.flavours and not config.receiver.url and not sum(config.leds.as_settings().values()):
        return 'tv: the local slim receiver drives WLED and needs the [leds] counts'
    return None


def _facts(ctx: Bench, **values: str) -> None:
    ctx.facts.update(values)


# ---- set-up and restore ------------------------------------------------------


def snapshot_wled(ctx: Ctx) -> tuple[str, object]:
    wled = wled_api.Wled(ctx.config.wled)
    saved = wled_api.snapshot(wled)
    path = ctx.save('wled-snapshot.json', saved)

    def restore_wled() -> str:
        left = wled_api.restore(wled, saved)
        if left:
            raise RuntimeError(f'WLED still differs from the snapshot: {left}')
        return f'restored from {path}'

    ctx.cleanup('tv.restore-wled', 'WLED restored to the snapshot', restore_wled)
    return f'saved {path}', saved


def preflight(ctx: Ctx) -> tuple[str, object]:
    ssh = ctx.config.tv.ssh
    ctx.run(commands.tv_reachable(ssh))
    running = tv.processes(ctx)
    if any('io.prysm.capture' in path for _, path in running):
        raise Failed('the old io.prysm.capture app is running; remove it first (docs/users/webos.md)', running)
    found = tv.get_state(ctx)
    present = tv.paths(ctx)
    if found is None and (present.get(commands.APP_DIR) or present.get(commands.SERVICE_DIR)):
        raise Failed(
            'Prysm is installed but its service does not answer getState: set it up or fix it by hand '
            'first, so the runner can put it back as it was',
            present,
        )
    backup = {name: tv.read_data(ctx, name) for name in commands.DATA_FILES}
    ctx.save('tv-data-backup.json', backup)
    as_found: AsFound = {
        'flavor': found['flavor'] if found else None,
        'preferences': found['preferences'] if found else None,
        'lighting': found.get('lighting') if found else None,
        'enabled': found.get('enabled') if found else None,
    }
    portal = found.get('portal') if found else None
    if found and found['flavor'] == Flavour.FULL and portal:
        try:
            as_found['settings'] = prysm_api.Prysm(portal).state()['settings']
        except (web.Unreachable, RuntimeError):
            as_found['settings'] = None
    ctx.data['as_found'] = as_found
    # The runner never builds: the package left at the end must be there before anything changes.
    restored = left_installed(ctx.config, as_found)
    if restored and not Path(ctx.config.tv.ipk(restored)).is_file():
        raise Failed(f'{ctx.config.tv.ipk(restored)} is missing, and the end of the run installs {restored} again')
    ctx.save('tv-as-found.json', as_found)
    _facts(ctx, **{'TV package found': as_found['flavor'] or 'none', 'TV': ssh})
    ctx.cleanup('tv.restore', 'TV left as found (package, preferences, settings, lighting)', lambda: restore(ctx))
    return f'found {as_found["flavor"] or "no Prysm"}; data backed up', {
        'processes': running,
        'state': found,
        'paths': present,
    }


def left_installed(config: Config, as_found: AsFound) -> Flavour | None:
    """The package to leave installed at the end (tv.restore_flavour); None: none."""
    match config.tv.restore_flavour:
        case RestoreFlavour.AS_FOUND:
            return as_found['flavor']
        case RestoreFlavour.NONE:
            return None
        case RestoreFlavour.FULL:
            return Flavour.FULL
        case RestoreFlavour.SLIM:
            return Flavour.SLIM
        case _:
            assert_never(config.tv.restore_flavour)


def restore(ctx: Ctx) -> str:
    as_found = ctx.data['as_found']
    flavour = left_installed(ctx.config, as_found)
    current = tv.get_state(ctx)
    if flavour is None:
        return 'nothing to restore' if current is None else 'left the last package installed'
    if current is None or current['flavor'] != flavour:
        if flavour not in ctx.data.get('ipk', {}):
            package(flavour)(ctx)
        tv.install(ctx, ctx.data['ipk'][flavour])
    preferences = as_found['preferences'] or {'autostart': False}
    tv.call(
        ctx,
        commands.tv_set_preferences(ctx.config.tv.ssh, bool(preferences.get('autostart')), preferences.get('instance')),
    )
    if flavour == Flavour.FULL:
        tv_full.wait_portal(ctx)
        prysm = tv_full.portal(ctx)
        settings = as_found.get('settings')
        if settings:
            reply = prysm.replace_settings(settings)
            if reply.status != 200:
                raise RuntimeError(f'settings not restored: HTTP {reply.status}')
        target = as_found.get('lighting') or Target.OFF
        reply = prysm.set_target(target)
        if reply.status != 204:
            raise RuntimeError(f'lighting {target} not restored: HTTP {reply.status}')
        return f'full installed, settings and preferences restored, lighting {target}'
    tv.call(ctx, commands.tv_set_enabled(ctx.config.tv.ssh, bool(as_found.get('enabled'))))
    return f'{flavour} installed, preferences restored, enabled {bool(as_found.get("enabled"))}'


def package(flavour: Flavour) -> Run:
    """The configured package: present, its hash and manifest recorded (`tv.install` verifies it)."""

    def run(ctx: Ctx) -> tuple[str, object]:
        ipk = Path(ctx.config.tv.ipk(flavour))
        if not ipk.is_file():
            raise Failed(f'{ipk} does not exist: cargo xtask dist tv-{flavour}, or set tv.ipk_{flavour}')
        found = artifact.inspect(ipk)
        ctx.data.setdefault('ipk', {})[flavour] = str(ipk)
        facts = found.facts()
        _facts(ctx, **{f'Package {flavour}': ', '.join(f'{key} {value}' for key, value in facts.items())})
        return f'{ipk.name}: {found.version or "version unknown"}', facts

    return run


def install(flavour: Flavour) -> Run:
    def run(ctx: Ctx) -> tuple[str, object]:
        state = tv.install(ctx, ctx.data['ipk'][flavour])
        return f'installed, setup ran; flavour {state["flavor"]}', state

    return run


# ---- both packages -----------------------------------------------------------


def registered(flavour: Flavour) -> Run:
    def run(ctx: Ctx) -> tuple[str, object]:
        state = tv.wait_state(ctx, lambda s: s['worker']['phase'] in ('running', 'stopped', 'retrying'), 20)
        if state is None or state['flavor'] != flavour:
            raise Failed(f'getState does not answer as {flavour}', state)
        present = tv.paths(ctx)
        missing = [p for p in commands.INSTALLED_PATHS if not present.get(p)]
        if missing:
            raise Failed(f'missing after setup: {missing}', present)
        return f'registered; worker {state["worker"]["phase"]}; all files present', {'state': state, 'paths': present}

    return run


def _running(flavour: Flavour) -> Callable[[Ctx], object]:
    """Lighting (full) or the stream to the receiver (slim) on, with frames."""
    match flavour:
        case Flavour.FULL:
            return tv_full.start
        case Flavour.SLIM:
            return tv_slim.stream
        case _:
            assert_never(flavour)


def standby(flavour: Flavour) -> Run:
    def run(ctx: Ctx) -> tuple[str, object]:
        _running(flavour)(ctx)
        answer, note = ctx.ask(
            "Put the TV in standby with the remote's power button (do not unplug). Answer y once the screen is off.",
            (Answer.YES, Answer.SKIP),
        )
        if answer != Answer.YES:
            raise Skipped(f'operator skipped standby {note}'.strip())
        asleep = tv.wait_state(ctx, lambda s: s['power'] != 'active', 60, every=3)
        evidence: dict[str, object] = {'standby_state': asleep}
        answer, note = ctx.ask(
            'Wake the TV with the power button and start video. Answer y when the picture is back.',
            (Answer.YES, Answer.SKIP),
        )
        if answer != Answer.YES:
            raise Skipped(f'operator skipped wake {note}'.strip(), evidence)
        awake = tv.wait_state(
            ctx, lambda s: s['power'] == 'active' and s['worker']['phase'] == 'running' and s['capturing'], 90, every=3
        )
        evidence['wake_state'] = awake
        if asleep is None:
            evidence['note'] = 'no standby state was observed over ssh (the TV may stop answering in standby)'
        if awake is None:
            raise Failed('capture did not resume within 90 s of waking', evidence)
        return f'standby seen: {asleep["power"] if asleep else "no answer"}; capture resumed', evidence

    return run


def reinstall(flavour: Flavour) -> Run:
    def run(ctx: Ctx) -> tuple[str, object]:
        _running(flavour)(ctx)
        before = tv.get_state(ctx)
        settings = tv_full.portal(ctx).state()['settings'] if flavour == Flavour.FULL else None
        tv.install(ctx, ctx.data['ipk'][flavour])
        after = tv.wait_state(ctx, lambda s: s['worker']['phase'] == 'running', 60)
        evidence: dict[str, object] = {'before': before, 'after': after}
        if after is None:
            raise Failed('the worker is not running 60 s after reinstall and setup', evidence)
        if after['preferences'] != cast('tv.LunaReply', before)['preferences']:
            raise Failed('preferences changed by the reinstall', evidence)
        if flavour == Flavour.FULL:
            evidence['settings_kept'] = tv_full.wait_portal(ctx)['settings'] == settings
            if not evidence['settings_kept']:
                raise Failed('settings.json changed by the reinstall', evidence)
            if not tv_full.resumed(ctx):
                raise Failed('lighting did not resume after the reinstall', evidence)
            return 'preferences and settings kept, lighting resumed', evidence
        if tv.wait_state(ctx, lambda s: s['capturing'], 30) is None:
            raise Failed('capture did not resume after the reinstall', evidence)
        return 'preferences kept, capture resumed', evidence

    return run


def uninstall(ctx: Bench) -> tuple[str, object]:
    ctx.run(commands.tv_supervisor_uninstall(ctx.config.tv.ssh))
    how = tv.remove_app(ctx)
    time.sleep(3)
    present = tv.paths(ctx)
    left = [p for p in (*commands.INSTALLED_PATHS, commands.STAGING) if present.get(p)]
    running = tv.processes(ctx)
    answering = tv.get_state(ctx)
    evidence = {
        'paths': present,
        'processes': running,
        'getState': answering,
        'removed_by': how,
        'checked': [*commands.INSTALLED_PATHS, commands.STAGING, 'processes under io.prysm.*', 'getState answers'],
        'kept_by_design': commands.DATA_DIR + ' (preferences and settings stay, docs/users/webos.md)',
    }
    if left or running or answering:
        raise Failed(
            f'left behind: {left or ""} {running or ""} {"getState answers" if answering else ""}'.strip(), evidence
        )
    return f'nothing left; {commands.DATA_DIR} kept by design', evidence


# ---- the plan ----------------------------------------------------------------


def steps(config: Config, options: Options) -> list[Step[TvData]]:
    ssh = config.tv.ssh or 'root@TV'
    state = [commands.tv_get_state(ssh)]
    found: list[Step[TvData]] = [
        Step(
            'tv.wled-snapshot',
            'WLED snapshot',
            'GET /json/si and /json/cfg saved; restored last',
            Mode.SETUP,
            snapshot_wled,
            actions=[f'GET http://{config.wled}/json/si', f'GET http://{config.wled}/json/cfg'],
        ),
        Step(
            'tv.preflight',
            'TV reachable, state saved',
            'ssh answers; no old io.prysm.capture running; an installed '
            f"Prysm answers getState; {commands.DATA_DIR} files and the full portal's settings saved locally",
            Mode.SETUP,
            preflight,
            commands=lambda: [
                commands.tv_reachable(ssh),
                commands.tv_processes(ssh),
                *state,
                commands.tv_paths(ssh),
                *(commands.tv_read_data(ssh, name) for name in commands.DATA_FILES),
            ],
            actions=['GET PORTAL/api/v1/state (full; PORTAL as getState reports it)'],
        ),
    ]
    if Flavour.SLIM in config.tv.flavours:
        found.append(
            Step(
                'tv.receiver',
                'Slim receiver',
                'receiver.url (settings saved and put back), or [mac] binary '
                'started here with the remote capture, WLED output and --allow-origin null',
                Mode.SETUP,
                tv_slim.start_receiver,
                group=Flavour.SLIM,
                commands=lambda: (
                    []
                    if config.receiver.url
                    else [
                        commands.headless(
                            config.mac.binary or 'MAC_BINARY',
                            'RUN_DIR/receiver-settings.json',
                            config.receiver.listen,
                            allow_null_origin=True,
                        )
                    ]
                ),
            )
        )
    for flavour in config.tv.flavours:
        found += _flavour_steps(config, options, flavour, ssh)
    return found


def cleanup_commands(config: Config) -> list[Command]:
    """The commands the restore may run, for the dry run."""
    ssh = config.tv.ssh or 'root@TV'
    ipk = config.tv.ipk_full  # or slim, as found
    return [
        commands.tv_get_state(ssh),
        *_install_commands(ssh, ipk),
        commands.tv_set_preferences(ssh, False),
        commands.tv_set_enabled(ssh, False),
    ]


def _install_commands(ssh: str, ipk: str) -> list[Command]:
    return [
        commands.verify_ipk(ipk),
        commands.tv_stage(ssh, ipk),
        commands.tv_install(ssh),
        commands.tv_unstage(ssh),
        commands.tv_setup(ssh),
        commands.tv_get_state(ssh),
    ]


def _flavour_steps(config: Config, options: Options, flavour: Flavour, ssh: str) -> list[Step[TvData]]:
    ipk = config.tv.ipk_full if flavour == Flavour.FULL else config.tv.ipk_slim
    prefix = f'tv.{flavour}.'
    state = [commands.tv_get_state(ssh)]
    sample = [commands.tv_sample_worker(ssh)]
    point = [commands.tv_set_preferences(ssh, False, EXAMPLE_RECEIVER)]

    def step(
        name: str,
        title: str,
        how: str,
        mode: Mode,
        run: Run | None,
        tv_commands: Sequence[Command] = (),
        actions: Iterable[str] = (),
        setup_group: bool = False,
    ) -> Step[TvData]:
        return Step(
            prefix + name,
            title,
            how,
            mode,
            run,
            commands=(lambda: list(tv_commands)) if tv_commands else None,
            actions=list(actions),
            group=flavour if setup_group else None,
        )

    found = [
        Step(
            prefix + 'package',
            f'{flavour} package',
            f'tv.ipk_{flavour} ({ipk}) exists; its SHA-256, and the version and commit from manifest.json '
            'next to it when the hash matches, are recorded',
            Mode.SETUP,
            package(flavour),
            group=flavour,
        ),
        step(
            'install',
            f'Install {flavour}',
            'ipk.py verify accepts it (no files outside the app and service directories); staged copy, '
            'appInstallService replies "installed", staged copy removed, setup.sh',
            Mode.SETUP,
            install(flavour),
            _install_commands(ssh, ipk),
            setup_group=True,
        ),
        step(
            'registered',
            'Service registered',
            'getState answers with this flavour; app, service, manifests and boot hook present',
            Mode.AUTO,
            registered(flavour),
            [*state, commands.tv_paths(ssh)],
        ),
    ]
    if flavour == Flavour.FULL:
        found += [
            step(
                'portal',
                'TV portal',
                'the TV portal answers within 40 s; its settings are saved and put back after '
                'the checks that change them',
                Mode.SETUP,
                tv_full.portal_step,
                actions=['GET PORTAL/api/v1/state (PORTAL as getState reports it)'],
                setup_group=True,
            ),
            step(
                'capture',
                'Capture',
                'target processed: running with frames (video must play); getState capturing',
                Mode.AUTO,
                tv_full.capture,
                state,
                ['PUT /api/v1/target processed'],
            ),
        ]
        found += [
            step(
                f'soak-{w}x{h}',
                f'Soak {w}x{h}@{f}',
                f'{config.tv.soak_seconds} s with lighting on: portal '
                f'fps within ±{config.checks.fps_tolerance:.0%}; worker CPU and RSS from /proc every 5 s; '
                'settings put back',
                Mode.AUTO,
                tv_full.soak(w, h, f),
                sample,
                [
                    f'PUT /api/v1/settings capture {w}x{h}@{f}',
                    'PUT /api/v1/target processed',
                    'PUT /api/v1/settings (the saved settings)',
                ],
            )
            for w, h, f in tv_full.SOAKS
        ]
        found.append(
            step(
                'modes',
                'Capture modes',
                'POST /api/v1/capture/modes {"kind":"webos"}: known',
                Mode.AUTO,
                tv_full.capture_modes,
            )
        )
    else:
        found += [
            step(
                'connect',
                'TV streams to the receiver',
                'receiver off; setPreferences instance and setEnabled true (as the app, -a io.prysm.tv)',
                Mode.SETUP,
                tv_slim.connect,
                [*point, commands.tv_set_enabled(ssh, True)],
                [f'instance: the receiver as the TV reaches it (shown as {EXAMPLE_RECEIVER})'],
                setup_group=True,
            ),
            step(
                'offer',
                'Offer with lights off',
                'receiver target off; POST /api/v1/capture/modes for each encoding is known within 30 s',
                Mode.AUTO,
                tv_slim.offer_while_off,
            ),
            step(
                'encodings',
                'Each encoding at 30 fps',
                '640x360@30 for h264_hardware, h264_software, raw: receiver '
                'fps within tolerance, getState encoding matches; TV worker CPU',
                Mode.AUTO,
                tv_slim.encodings,
                [*state, *sample],
            ),
            step(
                'soak-640x360',
                'Soak 640x360@30 (hardware H.264)',
                f'{config.tv.soak_seconds} s: receiver fps '
                f'within ±{config.checks.fps_tolerance:.0%}; TV worker CPU and RSS every 5 s',
                Mode.AUTO,
                tv_slim.soak,
                sample,
            ),
            step(
                'pause-resume',
                'Pause and resume',
                'receiver off: TV paused, not capturing; processed: capturing again',
                Mode.AUTO,
                tv_slim.pause_resume,
                state,
            ),
            step(
                'wss',
                'wss receiver',
                'instance https:// (receiver.wss_url): capturing with fps within 45 s; instance put back'
                if config.receiver.wss_url
                else 'not run: give receiver.wss_url or --wss-url',
                Mode.AUTO if config.receiver.wss_url else Mode.SKIP,
                tv_slim.wss,
                [*point, *state],
            ),
        ]
    if options.sweep:
        found.append(
            step(
                'sweep',
                'Size and rate sweep (prysm-884)',
                f'{config.sweep.sizes} x {config.sweep.rates} '
                'fps'
                + (' for capture' if flavour == Flavour.FULL else ' for each encoding')
                + f', {config.sweep.seconds} s each: delivered fps and TV CPU into sweep-{flavour}.csv',
                Mode.AUTO,
                tv_full.sweep if flavour == Flavour.FULL else tv_slim.sweep,
                sample,
            )
        )
    found += [
        step(
            'standby',
            'Standby and wake',
            'lighting on; a person presses power; getState shows standby (if the TV '
            'answers), then active with the worker running and capturing within 90 s of waking',
            Mode.GUIDED,
            standby(flavour),
            state,
        ),
        step(
            'reinstall',
            'Reinstall keeps settings',
            'lighting on; the same IPK installed again + setup: preferences '
            '(and full settings.json) unchanged; lighting or capture resumes',
            Mode.AUTO,
            reinstall(flavour),
            [*state, *_install_commands(ssh, ipk)],
        ),
        step(
            'uninstall',
            'Uninstall leaves nothing',
            'supervisor uninstall, then ares-remove (or the person removes '
            f'the app); absent: {", ".join(commands.INSTALLED_PATHS)}, the staged package, any io.prysm.* process, '
            f'the Luna service; {commands.DATA_DIR} stays by design',
            Mode.AUTO,
            uninstall,
            [
                commands.tv_supervisor_uninstall(ssh),
                *([commands.ares_remove(config.tv.ares_device)] if config.tv.ares_device else []),
                commands.tv_paths(ssh),
                commands.tv_processes(ssh),
                *state,
            ],
        ),
    ]
    return found
