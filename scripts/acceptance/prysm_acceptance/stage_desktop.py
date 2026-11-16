"""Stage A: prysm-headless on this computer with a file or a capture card, driving WLED."""

import shutil
import time
from typing import assert_never

from prysm_acceptance import colour_page, commands, edges_clip, feed, host, modes, prysm_api, wled_api
from prysm_acceptance.commands import Command
from prysm_acceptance.config import Config
from prysm_acceptance.edges_clip import CLIP
from prysm_acceptance.feed import Feed
from prysm_acceptance.options import Options, Source
from prysm_acceptance.plan import Answer, Context, Failed, Mode, Skipped, Step
from prysm_acceptance.prysm_api import ColorsEvent, Size, Target, near, rates, section, session
from prysm_acceptance.stage_data import DesktopData
from prysm_acceptance.web import JsonObject

Ctx = Context[DesktopData]

CLEANUPS = (
    'prysm-headless stopped with SIGTERM (KILL after 30 s)',
    'WLED restored from wled-snapshot.json (live off, preset, on, bri, lor, transition, if.live.maxbri) '
    'and compared; by hand: acceptance restore-wled RUN_DIR/wled-snapshot.json',
)


def _feed(config: Config, options: Options) -> Feed | None:
    """The feed this run shows the clip on, or None.

    Only a capture card has an input to feed. `--display-check` says the card
    records this computer's display, so the feed is not used then."""
    if options.source != Source.DEVICE or options.display_check:
        return None
    return feed.configured(config)


def cleanups(config: Config, options: Options) -> tuple[str, ...]:
    """What the end of the run undoes, in that order, for the dry run."""
    controlled = _feed(config, options)
    return CLEANUPS[:1] + (controlled.CLEANUPS if controlled else ()) + CLEANUPS[1:]


def _prysm(ctx: Ctx) -> prysm_api.Prysm:
    return ctx.data['prysm']


def _wled(ctx: Ctx) -> wled_api.Wled:
    return ctx.data['wled']


def _expected_fps(ctx: Ctx) -> int:
    return ctx.data.get('fps', CLIP['fps'])


# ---- setup -----------------------------------------------------------------


def preflight(ctx: Ctx) -> str:
    config, options = ctx.config, ctx.options
    if not config.wled:
        raise Failed('wled.address is not set (config file or --wled)')
    if sum(config.leds.as_settings().values()) == 0:
        raise Failed('leds: no LEDs configured')
    controlled = _feed(config, options)
    if controlled:
        controlled.preflight()
    if (options.source == Source.FILE or controlled) and not shutil.which('ffmpeg'):
        raise Failed('the test clip needs FFmpeg 9+ on PATH')
    ctx.data['wled'] = wled_api.Wled(config.wled)
    address = host.local_address(config.wled)
    ctx.data['local_address'] = address
    return f'source {options.source}, WLED {config.wled}, this computer {address}'


def snapshot_wled(ctx: Ctx) -> tuple[str, object]:
    wled = _wled(ctx)
    saved = wled_api.snapshot(wled)
    if saved['info'].get('live'):
        # Two senders would fight over the LEDs and no check could be trusted.
        raise Failed(
            f'WLED is already driven by {saved["info"].get("lip")}: turn that lighting off first', saved['info']
        )
    path = ctx.save('wled-snapshot.json', saved)
    ctx.data['wled_snapshot'] = saved

    def restore() -> str:
        left = wled_api.restore(wled, saved)
        if left:
            raise RuntimeError(f'WLED still differs from the snapshot: {left}')
        return f'restored from {path}'

    ctx.cleanup('desktop.restore-wled', 'WLED restored to the snapshot', restore)
    return f'saved {path}', saved


def _clip_command(path: str) -> Command:
    return commands.ffmpeg_edges_clip(
        path,
        CLIP['width'],
        CLIP['height'],
        CLIP['fps'],
        CLIP['seconds'],
        edges_clip.corner_hex(),
        edges_clip.STEP_SECONDS,
    )


def clip(ctx: Ctx) -> str:
    if ctx.options.source_path:
        ctx.data['clip'] = ctx.options.source_path
        return f'using {ctx.options.source_path} (colour check off: unknown content)'
    path = ctx.out_dir / 'edges.mp4'
    ctx.run(_clip_command(str(path)))
    ctx.data['clip'] = str(path)
    ctx.data['clip_edges'] = True
    return f'generated {path}'


def show_clip(ctx: Ctx, controlled: Feed) -> tuple[str, object]:
    """Show the clip on the feed; the feed registers what stops it."""
    detail, evidence = controlled.show(ctx, ctx.data['clip'])
    ctx.data['feed'] = controlled
    return detail, evidence


def _feed_settled(ctx: Ctx) -> None:
    """With a feed showing the clip: wait until its picture can be trusted."""
    showing = ctx.data.get('feed')
    if showing:
        showing.settled()


def write_settings(ctx: Ctx) -> str:
    document = host.settings(host.wled_output(ctx.config))
    path = host.write_settings(ctx, 'settings.json', document)
    result = ctx.run(commands.check_settings(host.binary(ctx.config), str(path)))
    if 'settings OK' not in result.stdout + result.stderr:
        raise Failed('--check-settings did not say settings OK', result.stdout + result.stderr)
    ctx.data['settings_path'] = path
    return f'{path}: settings OK'


def start(ctx: Ctx) -> str:
    headless = host.Headless(ctx, str(ctx.data['settings_path']), ctx.config.desktop.listen)
    ctx.cleanup('desktop.stop-headless', 'prysm-headless stopped', lambda: f'exit code {headless.stop()}')
    state = headless.start()
    ctx.data['headless'] = headless
    ctx.data['prysm'] = prysm_api.Prysm(headless.url)
    return f'{headless.url}: target {state["target"]}, session {session(state)}'


def source(ctx: Ctx) -> tuple[str, object]:
    prysm = _prysm(ctx)
    match ctx.options.source:
        case Source.FILE:
            size: Size = {'width': CLIP['width'], 'height': CLIP['height'], 'fps': CLIP['fps']}
            capture: JsonObject = {
                'kind': 'file',
                'path': ctx.data['clip'],
                'width': size['width'],
                'height': size['height'],
                'fps': size['fps'],
                'repeat': True,
            }
            evidence: object = {'capture': capture}
        case Source.DEVICE:
            path, size, evidence = _device_capture(ctx, prysm)
            capture = {
                'kind': 'device',
                'path': path,
                'width': size['width'],
                'height': size['height'],
                'fps': size['fps'],
            }
        case _:
            assert_never(ctx.options.source)
    reply = prysm.change_settings(lambda settings: settings.__setitem__('capture', capture))
    if reply.status != 200:
        raise Failed(f'PUT /api/v1/settings: HTTP {reply.status}', reply.json())
    ctx.data['capture'] = capture
    ctx.data['fps'] = size['fps']
    return f'{capture["kind"]} {capture["width"]}x{capture["height"]} @ {capture["fps"]}', evidence


def _device_capture(ctx: Ctx, prysm: prysm_api.Prysm) -> tuple[str, Size, object]:
    """The capture device to use, its size and rate (the preset resolved) and what was found."""
    found = prysm.candidates('capture', 'device')
    ctx.log('Capture devices: ' + (', '.join(f'{c["label"]} ({c["fields"].get("path")})' for c in found) or 'none'))
    wanted = ctx.config.desktop.device.lower()
    chosen = [c for c in found if wanted in c['label'].lower()] if wanted else found
    if not chosen:
        raise Failed(f'no capture device{" matching " + repr(wanted) if wanted else ""}', found)
    device = chosen[0]
    path = device['fields']['path']
    reply = prysm.modes({'kind': 'device', 'path': path})
    answer = prysm_api.modes_answer(reply)
    if reply.status != 200 or answer.get('status') != 'known':
        raise Failed(f'capture modes of {device["label"]} unknown (HTTP {reply.status})', answer)
    ctx.log('Modes: ' + '; '.join(modes.describe(answer['modes'])))
    size = modes.resolve(answer['modes'], modes.PRESETS[ctx.config.desktop.preset])
    if size is None:
        raise Failed(f'no mode covers {ctx.config.desktop.preset}', answer)
    return (
        path,
        size,
        {
            'devices': found,
            'modes': answer['modes'],
            'preset': ctx.config.desktop.preset,
            'resolved': size,
        },
    )


# ---- checks ----------------------------------------------------------------


def _start_processed(ctx: Ctx, seconds: float = 15) -> prysm_api.State:
    prysm = _prysm(ctx)
    reply = prysm.set_target(Target.PROCESSED)
    if reply.status != 204:
        raise Failed(f'PUT /api/v1/target processed: HTTP {reply.status}', reply.json())
    state = prysm.wait(prysm_api.running, seconds)
    if state is None:
        raise Failed(f'not running with frames within {seconds} s', prysm.state())
    return state


def processed(ctx: Ctx) -> tuple[str, object]:
    state = _start_processed(ctx)
    return f'running, {(state["input"] or {})["fps"]} fps', state['input']


def wled_live(ctx: Ctx) -> tuple[str, object]:
    _start_processed(ctx)
    expected = ctx.data['local_address']
    si = _wled(ctx).wait(lambda s: s['info'].get('live') and s['info'].get('lm') == 'DDP', 10)
    if si is None:
        raise Failed('WLED did not report live DDP within 10 s', _wled(ctx).si()['info'])
    if si['info'].get('lip') not in (expected, '127.0.0.1'):
        raise Failed(f'WLED shows another sender: {si["info"].get("lip")} (this computer: {expected})', si['info'])
    issue = _prysm(ctx).state().get('output_issue')
    if issue:
        raise Failed(f'Prysm reports output_issue {issue}')
    return f'live DDP from {si["info"]["lip"]}', si['info']


def fps(ctx: Ctx) -> tuple[str, object]:
    expected = _expected_fps(ctx)
    tolerance = ctx.config.desktop.fps_tolerance
    _start_processed(ctx)
    measured = rates(_prysm(ctx).sample(ctx.config.desktop.seconds))
    measured['expected'] = expected
    if not near(measured['fps_mean'], expected, tolerance):
        raise Failed(f'input {measured["fps_mean"]} fps, expected {expected} ±{tolerance:.0%}', measured)
    if measured['sent_mean'] < expected * (1 - tolerance):
        raise Failed(f'sent {measured["sent_mean"]} fps, expected about {expected}', measured)
    if measured['behind']:
        raise Failed(f'Prysm fell behind: {measured["behind"]}', measured)
    return f'{measured["fps_mean"]} fps in, {measured["sent_mean"]} sent (min {measured["fps_min"]})', measured


def _last_colours(ctx: Ctx, seconds: float = 3.0) -> ColorsEvent:
    found = [event for event in _prysm(ctx).colors(seconds) if event.get('type') == 'colors']
    if not found:
        raise Failed(f'no colours event within {seconds} s')
    return found[-1]


def colours(ctx: Ctx) -> tuple[str, object]:
    _start_processed(ctx)
    _feed_settled(ctx)
    if ctx.data.get('clip_edges'):
        seconds = edges_clip.CHECK_SECONDS
        return edges_clip.judge(_prysm(ctx).colors(seconds, timed=True), seconds)
    event = _last_colours(ctx)
    brightest = max(max(sum(c) for c in event.get(edge) or [[0, 0, 0]]) for edge in prysm_api.EDGES)
    if brightest < 30:
        raise Failed('every edge is black', event)
    return f'not black (brightest edge sum {brightest})', event


def device_native(ctx: Ctx) -> tuple[str, object]:
    prysm, wled = _prysm(ctx), _wled(ctx)
    reply = prysm.set_target(Target.DEVICE_NATIVE)
    if reply.status != 204:
        raise Failed(f'PUT /api/v1/target device_native: HTTP {reply.status}', reply.json())
    state = prysm.wait(lambda s: session(s) == 'native', 10)
    si = wled.wait(lambda s: not s['info'].get('live') and s['state'].get('on'), 10)
    if state is None or si is None:
        raise Failed('Prysm not native or WLED still live/off', {'prysm': prysm.state(), 'wled': wled.si()})
    return 'Prysm native, WLED on and not live', si['state']


def processed_again(ctx: Ctx) -> tuple[str, object]:
    _start_processed(ctx)
    return wled_live(ctx)


def off(ctx: Ctx) -> tuple[str, object]:
    prysm, wled = _prysm(ctx), _wled(ctx)
    reply = prysm.set_target(Target.OFF)
    if reply.status != 204:
        raise Failed(f'PUT /api/v1/target off: HTTP {reply.status}', reply.json())
    state = prysm.wait(lambda s: session(s) == 'stopped', 10)
    si = wled.wait(lambda s: not s['state'].get('on'), 10)
    if state is None or si is None:
        raise Failed('Prysm not stopped or WLED still on', {'prysm': prysm.state(), 'wled': wled.si()})
    return 'Prysm stopped, WLED off', si['state']


def brightness(ctx: Ctx) -> tuple[str, object]:
    """WLED 16.0.1's brightness chain (prysm-3jx.74): the note at 40 %, gone with Force max."""
    prysm, wled = _prysm(ctx), _wled(ctx)
    raw = wled.cfg_raw()
    config = wled_api.cfg(raw)
    evidence: dict[str, object] = {
        'cfg_bytes': len(raw),
        'maxbri': wled_api.lookup(config, 'if', 'live', 'maxbri'),
        'scale_bri': wled_api.lookup(config, 'light', 'scale-bri'),
    }
    if len(raw) >= 65536:
        raise Failed('/json/cfg is 64 KiB or more', evidence)
    if evidence['maxbri'] is None or evidence['scale_bri'] is None:
        raise Failed('/json/cfg lacks if.live.maxbri or light.scale-bri', evidence)
    _start_processed(ctx)
    saved = ctx.data['wled_snapshot']
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
        wled.post_cfg({'if': {'live': {'maxbri': saved['cfg']['maxbri']}}})
        wled.post_state({'bri': saved['state'].get('bri', 128)})
    return (
        f'40 % after {evidence["note_40_seconds"]} s, gone {evidence["force_max_seconds"]} s after Force max'
    ), evidence


def gamma(ctx: Ctx) -> tuple[str, object]:
    """WLED's gamma fields where Prysm reads them; read-only (see wled_api.gamma_fields)."""
    found, problems = wled_api.gamma_fields(wled_api.cfg(_wled(ctx).cfg_raw()))
    version = ctx.data['wled_snapshot']['info'].get('ver')
    if problems:
        raise Failed('; '.join(problems), {'ver': version, **found})
    return f'WLED {version}: {wled_api.describe_gamma(found)}', {'ver': version, **found}


def dither(ctx: Ctx) -> tuple[str, object]:
    prysm = _prysm(ctx)
    _start_processed(ctx)
    found: dict[str, prysm_api.Rates] = {}
    for value in (False, True):
        reply = prysm.change_settings(lambda s, value=value: section(s, 'output').__setitem__('dither', value))
        if reply.status != 200:
            raise Failed(f'dither {value}: HTTP {reply.status}', reply.json())
        state = prysm.wait(
            lambda s, value=value: prysm_api.running(s) and section(s['settings'], 'output').get('dither') == value,
            15,
        )
        if state is None:
            raise Failed(f'not running again with dither {value}', prysm.state())
        found[str(value)] = rates(prysm.sample(3))
    return 'accepted off and on, lighting kept running', found


def release(ctx: Ctx) -> tuple[str, object]:
    prysm = _prysm(ctx)
    off(ctx)
    state = _start_processed(ctx, seconds=20)
    measured = rates(prysm.sample(ctx.config.desktop.seconds))
    if measured['fps_mean'] <= 0:
        raise Failed('no frames after re-opening', measured)
    _feed_settled(ctx)
    return f're-opened: {measured["fps_mean"]} fps', {'first': state['input'], 'rates': measured}


def display_check(ctx: Ctx) -> tuple[str, object]:
    if ctx.options.source != Source.DEVICE:
        raise Skipped("needs --source device with this Mac as the card's input")
    page = colour_page.ColourPage(ctx.config.desktop.display_port).start()
    try:
        answer, note = ctx.ask(
            f'Open {page.url} in a browser on the display the capture card records '
            "(this Mac's HDMI output), press F for full screen. Ready?",
            (Answer.YES, Answer.SKIP),
        )
        if answer != Answer.YES:
            raise Skipped(f'operator skipped the display set-up {note}'.strip())
        _start_processed(ctx)
        results: dict[str, object] = {}
        ok = True
        for name, edges in colour_page.FIELDS:
            if not page.show(name, edges):
                raise Failed(f'the page did not show {name} within 10 s (is it open?)', results)
            time.sleep(1.5)
            event = _last_colours(ctx, 2.0)
            matched, means = colour_page.matches(event, edges)
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
        return f'{len(results)} fields, each edge the expected colour', results
    finally:
        page.close()


def sigterm(ctx: Ctx) -> tuple[str, object]:
    headless, wled = ctx.data['headless'], _wled(ctx)
    _start_processed(ctx)
    start = time.monotonic()
    code = headless.stop()
    seconds = round(time.monotonic() - start, 1)
    if code != 0:
        raise Failed(f'exit code {code} after SIGTERM', {'seconds': seconds})
    si = wled.wait(lambda s: not s['info'].get('live'), 10)
    if si is None:
        raise Failed('WLED still live after Prysm exited', wled.si()['info'])
    return f'exit 0 after {seconds} s, WLED released', {'seconds': seconds, 'wled': si['state']}


def _feed_step(controlled: Feed | None, options: Options) -> Step[DesktopData]:
    """The setup step that puts the clip into the capture card, or a skip that says why nothing does."""
    if controlled:
        return Step(
            'desktop.feed',
            'Picture into the capture card',
            controlled.how(),
            Mode.SETUP,
            lambda ctx: show_clip(ctx, controlled),
            actions=controlled.actions(),
            commands=controlled.commands,
        )
    why = (
        "--display-check: the card records this computer's display, so no feed is started"
        if options.display_check
        else 'no feed in the configuration ([feed]): the card records whatever is plugged into it'
    )
    return Step(
        'desktop.feed', 'Picture into the capture card', why + '; the colour check is only "not black"', Mode.SKIP
    )


def steps(config: Config, options: Options) -> list[Step[DesktopData]]:
    device = options.source == Source.DEVICE
    controlled = _feed(config, options)
    found: list[Step[DesktopData]] = [
        Step(
            'desktop.preflight',
            'Configuration and tools',
            "WLED address and LEDs set; FFmpeg for the test clip; the feed's tools",
            Mode.SETUP,
            preflight,
        ),
        Step(
            'desktop.build',
            'Release prysm-headless',
            'cargo build --release (or the configured binary)',
            Mode.SETUP,
            host.build,
            commands=lambda: [] if config.desktop.headless or options.no_build else [commands.cargo_build_headless()],
        ),
        Step(
            'desktop.wled-snapshot',
            'WLED snapshot',
            'GET /json/si and /json/cfg saved before anything changes; restored at the end, also on failure or Ctrl-C',
            Mode.SETUP,
            snapshot_wled,
            actions=[f'GET http://{config.wled}/json/si', f'GET http://{config.wled}/json/cfg'],
        ),
    ]
    if not device or controlled:
        found.append(
            Step(
                'desktop.clip',
                'Test clip',
                'a generated clip whose edge colours step round the corners '
                f'every {edges_clip.STEP_SECONDS} s, or --source file:PATH',
                Mode.SETUP,
                clip,
                commands=lambda: [] if options.source_path else [_clip_command('RUN_DIR/edges.mp4')],
            )
        )
    if device:
        found.append(_feed_step(controlled, options))
    found += [
        Step(
            'desktop.settings',
            'Temporary settings',
            'WLED output, neutral processing; --check-settings says OK',
            Mode.SETUP,
            write_settings,
            commands=lambda: [commands.check_settings(host.binary(config), 'RUN_DIR/settings.json')],
        ),
        Step(
            'desktop.start',
            'Start prysm-headless',
            'GET /api/v1/state answers',
            Mode.SETUP,
            start,
            commands=lambda: [commands.headless(host.binary(config), 'RUN_DIR/settings.json', config.desktop.listen)],
        ),
        Step(
            'desktop.source',
            'Video source',
            'device: candidates, modes and the resolved preset printed; PUT /api/v1/settings with the capture',
            Mode.SETUP,
            source,
            actions=['GET /api/v1/candidates/capture/device', 'POST /api/v1/capture/modes', 'PUT /api/v1/settings']
            if device
            else ['PUT /api/v1/settings (capture file, repeat)'],
        ),
        Step(
            'desktop.processed',
            'Screen sync starts',
            'PUT /api/v1/target processed is 204; session running, frames arriving within 15 s',
            Mode.AUTO,
            processed,
        ),
        Step(
            'desktop.wled-live',
            'WLED shows Prysm',
            'WLED /json/si: live, lm DDP, lip this computer; no output_issue',
            Mode.AUTO,
            wled_live,
        ),
        Step(
            'desktop.fps',
            'Frame rate',
            f'state.input over {config.desktop.seconds} s: fps within '
            f'±{config.desktop.fps_tolerance:.0%} of the source, sent_fps alike, never behind',
            Mode.AUTO,
            fps,
        ),
        Step(
            'desktop.colours',
            'Colours',
            f"colours stream for {edges_clip.CHECK_SECONDS:g} s: each edge's two halves "
            "are nearest the clip's two colours among nine named colours, in the right direction; at least two "
            f'steps, in order, {edges_clip.STEP_SECONDS} s apart (±{edges_clip.PACE_TOLERANCE} s); no step for at most '
            f'{edges_clip.TRANSITION} s at a change (a capture card without a feed: not black)',
            Mode.AUTO,
            colours,
        ),
        Step(
            'desktop.device-native',
            'WLED effects',
            'target device_native: session native; WLED on, not live',
            Mode.AUTO,
            device_native,
        ),
        Step(
            'desktop.processed-again',
            'Screen sync again',
            'target processed: running and WLED live DDP',
            Mode.AUTO,
            processed_again,
        ),
        Step('desktop.off', 'Lights off', 'target off: session stopped; WLED on=false', Mode.AUTO, off),
        Step(
            'desktop.brightness',
            'WLED brightness',
            'bri 102: output_brightness 40 % within 5 s; '
            'if.live.maxbri on: back to 100 % within 35 s, state.bri unchanged; /json/cfg under 64 KiB '
            '(WLED changes restored)',
            Mode.AUTO,
            brightness,
            actions=[
                'POST /json/state {"bri":102}',
                'POST /json/cfg {"if":{"live":{"maxbri":true}}}',
                'restore maxbri and bri',
            ],
        ),
        Step(
            'desktop.gamma',
            'WLED gamma',
            '/json/cfg has the gamma fields Prysm reads, as it parses them: '
            'if.live.no-gc true or false, light.gc.col and light.gc.bri numbers within 0.1–3. Proves the layout, '
            'not that Prysm applied the value (output-wled unit tests; stage eyes: dark scenes)',
            Mode.AUTO,
            gamma,
            actions=['GET /json/cfg (read-only)'],
        ),
        Step(
            'desktop.dither',
            'Smooth dark colours switch',
            'output.dither false then true: 200, lighting running again each time',
            Mode.AUTO,
            dither,
        ),
    ]
    if device:
        found.append(
            Step(
                'desktop.release',
                'Capture release',
                'off, then processed: the device opens again with frames',
                Mode.AUTO,
                release,
            )
        )
        if options.display_check:
            found.append(
                Step(
                    'desktop.display-check',
                    'Colour fields on the capture display',
                    'a local page shows red, green, blue, white and an edge pattern full screen; each '
                    "edge (middle half) is nearest the field's colour among nine named colours",
                    Mode.GUIDED,
                    display_check,
                    actions=[f'serve http://127.0.0.1:{config.desktop.display_port}/ (localhost only)'],
                )
            )
    found.append(
        Step('desktop.sigterm', 'Clean exit', 'SIGTERM while lighting: exit code 0; WLED released', Mode.AUTO, sigterm)
    )
    return found
