"""Stage A: prysm-headless on this computer with a file or a capture card, driving WLED."""
import json
from pathlib import Path
import shutil
import time

import colour_page
import commands
import feed
import host
import modes
from plan import Failed, Skipped, Step
import prysm_api
from prysm_api import near, rates, session
import wled_api

# The generated clip's edge colours (hex) and what the colours stream should report.
CLIP_EDGES = {'top': 'ff0000', 'right': '00ff00', 'bottom': '0000ff', 'left': 'ffff00'}
# 30 minutes, longer than the whole stage, so a feed that does not loop never
# reaches the end; about 8 s to encode and 2 MB.
CLIP = {'width': 640, 'height': 360, 'fps': 30, 'seconds': 1800}
CLEANUPS = ('prysm-headless stopped with SIGTERM (KILL after 30 s)',
            'WLED restored from wled-snapshot.json (live off, preset, on, bri, lor, transition, if.live.maxbri) '
            'and compared; by hand: run.py restore-wled RUN_DIR/wled-snapshot.json')


def _feed(config, options):
    """The feed this run shows the clip on, or None.

    Only a capture card has an input to feed. `--display-check` says the card
    records this computer's display, so the feed is not used then."""
    if options.source != 'device' or options.display_check:
        return None
    return feed.configured(config)


def cleanups(config, options):
    """What the end of the run undoes, in that order, for the dry run."""
    controlled = _feed(config, options)
    return CLEANUPS[:1] + (controlled.CLEANUPS if controlled else ()) + CLEANUPS[1:]


def _prysm(ctx):
    return ctx.data['prysm']


def _wled(ctx):
    return ctx.data['wled']


def _expected_fps(ctx):
    return ctx.data.get('fps', CLIP['fps'])


# ---- setup -----------------------------------------------------------------

def preflight(ctx):
    config, options = ctx.config, ctx.options
    if not config.wled:
        raise Failed('wled.address is not set (config file or --wled)')
    if sum(config.leds.as_settings().values()) == 0:
        raise Failed('leds: no LEDs configured')
    controlled = _feed(config, options)
    if controlled:
        controlled.preflight()
    if (options.source == 'file' or controlled) and not shutil.which('ffmpeg'):
        raise Failed('the test clip needs FFmpeg 9+ on PATH')
    ctx.data['wled'] = wled_api.Wled(config.wled)
    address = host.local_address(config.wled)
    ctx.data['local_address'] = address
    return f'source {options.source}, WLED {config.wled}, this computer {address}'


def build(ctx):
    binary = Path(host.binary(ctx.config))
    if ctx.config.desktop.headless or ctx.options.no_build:
        if not binary.exists():
            raise Failed(f'{binary} does not exist')
        return f'using {binary}'
    if not (commands.ROOT / 'frontend/dist/headless').is_dir():
        raise Failed('frontend/dist/headless is missing: run npm --prefix frontend ci && '
                     'npm --prefix frontend run build first')
    ctx.run(commands.cargo_build_headless())
    return f'built {binary}'


def snapshot_wled(ctx):
    wled = _wled(ctx)
    saved = wled_api.snapshot(wled)
    if saved['info'].get('live'):
        # Two senders would fight over the LEDs and no check could be trusted.
        raise Failed(f'WLED is already driven by {saved["info"].get("lip")}: turn that lighting off first',
                     saved['info'])
    path = ctx.save('wled-snapshot.json', saved)
    ctx.data['wled_snapshot'] = saved

    def restore():
        left = wled_api.restore(wled, saved)
        if left:
            raise RuntimeError(f'WLED still differs from the snapshot: {left}')
        return f'restored from {path}'
    ctx.cleanup('desktop.restore-wled', 'WLED restored to the snapshot', restore)
    return f'saved {path}', saved


def clip(ctx):
    if ctx.options.source_path:
        ctx.data['clip'] = ctx.options.source_path
        return f'using {ctx.options.source_path} (colour check off: unknown content)'
    path = ctx.out_dir / 'edges.mp4'
    ctx.run(commands.ffmpeg_edges_clip(str(path), CLIP['width'], CLIP['height'], CLIP['fps'],
                                       CLIP['seconds'], CLIP_EDGES))
    ctx.data['clip'] = str(path)
    ctx.data['clip_edges'] = True
    return f'generated {path}'


def show_clip(ctx, controlled):
    """Show the clip on the feed; the feed registers what stops it."""
    detail, evidence = controlled.show(ctx, ctx.data['clip'])
    ctx.data['feed'] = controlled
    return detail, evidence


def _feed_settled(ctx):
    """With a feed showing the clip: wait until its picture can be trusted."""
    showing = ctx.data.get('feed')
    if showing:
        showing.settled()


def write_settings(ctx):
    document = host.settings(host.wled_output(ctx.config))
    path = host.write_settings(ctx, 'settings.json', document)
    result = ctx.run(commands.check_settings(host.binary(ctx.config), str(path)))
    if 'settings OK' not in result.stdout + result.stderr:
        raise Failed('--check-settings did not say settings OK', result.stdout + result.stderr)
    ctx.data['settings_path'] = path
    return f'{path}: settings OK'


def start(ctx):
    headless = host.Headless(ctx, str(ctx.data['settings_path']), ctx.config.desktop.listen)
    ctx.cleanup('desktop.stop-headless', 'prysm-headless stopped',
                lambda: f'exit code {headless.stop()}')
    state = headless.start()
    ctx.data['headless'] = headless
    ctx.data['prysm'] = prysm_api.Prysm(headless.url)
    return f'{headless.url}: target {state["target"]}, session {session(state)}'


def source(ctx):
    prysm = _prysm(ctx)
    if ctx.options.source == 'file':
        capture = {'kind': 'file', 'path': ctx.data['clip'], 'width': CLIP['width'],
                   'height': CLIP['height'], 'fps': CLIP['fps'], 'repeat': True}
        evidence = {'capture': capture}
    else:
        capture, evidence = _device_capture(ctx, prysm)
    reply = prysm.change_settings(lambda settings: settings.__setitem__('capture', capture))
    if reply.status != 200:
        raise Failed(f'PUT /api/v1/settings: HTTP {reply.status}', reply.json())
    ctx.data['capture'] = capture
    ctx.data['fps'] = capture['fps']
    return f'{capture["kind"]} {capture["width"]}x{capture["height"]} @ {capture["fps"]}', evidence


def _device_capture(ctx, prysm):
    found = prysm.candidates('capture', 'device')
    ctx.log('Capture devices: ' + (', '.join(f'{c["label"]} ({c["fields"].get("path")})' for c in found) or 'none'))
    wanted = ctx.config.desktop.device.lower()
    chosen = [c for c in found if wanted in c['label'].lower()] if wanted else found
    if not chosen:
        raise Failed(f'no capture device{" matching " + repr(wanted) if wanted else ""}', found)
    device = chosen[0]
    path = device['fields']['path']
    reply = prysm.modes({'kind': 'device', 'path': path})
    answer = reply.json()
    if reply.status != 200 or answer.get('status') != 'known':
        raise Failed(f'capture modes of {device["label"]} unknown (HTTP {reply.status})', answer)
    ctx.log('Modes: ' + '; '.join(modes.describe(answer['modes'])))
    size = modes.resolve(answer['modes'], modes.PRESETS[ctx.config.desktop.preset])
    if size is None:
        raise Failed(f'no mode covers {ctx.config.desktop.preset}', answer)
    capture = {'kind': 'device', 'path': path, **size}
    return capture, {'devices': found, 'modes': answer['modes'], 'preset': ctx.config.desktop.preset,
                     'resolved': size}


# ---- checks ----------------------------------------------------------------

def _start_processed(ctx, seconds=15):
    prysm = _prysm(ctx)
    reply = prysm.set_target('processed')
    if reply.status != 204:
        raise Failed(f'PUT /api/v1/target processed: HTTP {reply.status}', reply.json())
    state = prysm.wait(prysm_api.running, seconds)
    if state is None:
        raise Failed(f'not running with frames within {seconds} s', prysm.state())
    return state


def processed(ctx):
    state = _start_processed(ctx)
    return f'running, {state["input"]["fps"]} fps', state['input']


def wled_live(ctx):
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


def fps(ctx):
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


def _last_colours(ctx, seconds=3.0):
    found = [event for event in _prysm(ctx).colors(seconds) if event.get('type') == 'colors']
    if not found:
        raise Failed(f'no colours event within {seconds} s')
    return found[-1]


def colours(ctx):
    _start_processed(ctx)
    _feed_settled(ctx)
    event = _last_colours(ctx)
    if ctx.data.get('clip_edges'):
        expected = {edge: tuple(int(hex_[i:i + 2], 16) for i in (0, 2, 4)) for edge, hex_ in CLIP_EDGES.items()}
        ok, means = colour_page.matches(event, expected)
        evidence = {'expected': expected, 'measured': means}
        if not ok:
            raise Failed('edge colours differ from the clip', evidence)
        return 'each edge shows the clip\'s colour', evidence
    brightest = max(max(sum(c) for c in event.get(edge) or [[0, 0, 0]]) for edge in ('top', 'right', 'bottom', 'left'))
    if brightest < 30:
        raise Failed('every edge is black', event)
    return f'not black (brightest edge sum {brightest})', event


def device_native(ctx):
    prysm, wled = _prysm(ctx), _wled(ctx)
    reply = prysm.set_target('device_native')
    if reply.status != 204:
        raise Failed(f'PUT /api/v1/target device_native: HTTP {reply.status}', reply.json())
    state = prysm.wait(lambda s: session(s) == 'native', 10)
    si = wled.wait(lambda s: not s['info'].get('live') and s['state'].get('on'), 10)
    if state is None or si is None:
        raise Failed('Prysm not native or WLED still live/off', {'prysm': prysm.state(), 'wled': wled.si()})
    return 'Prysm native, WLED on and not live', si['state']


def processed_again(ctx):
    _start_processed(ctx)
    return wled_live(ctx)


def off(ctx):
    prysm, wled = _prysm(ctx), _wled(ctx)
    reply = prysm.set_target('off')
    if reply.status != 204:
        raise Failed(f'PUT /api/v1/target off: HTTP {reply.status}', reply.json())
    state = prysm.wait(lambda s: session(s) == 'stopped', 10)
    si = wled.wait(lambda s: not s['state'].get('on'), 10)
    if state is None or si is None:
        raise Failed('Prysm not stopped or WLED still on', {'prysm': prysm.state(), 'wled': wled.si()})
    return 'Prysm stopped, WLED off', si['state']


def brightness(ctx):
    """WLED 16.0.1's brightness chain (prysm-3jx.74): the note at 40 %, gone with Force max."""
    prysm, wled = _prysm(ctx), _wled(ctx)
    raw = wled.cfg_raw()
    cfg = json.loads(raw)
    evidence = {'cfg_bytes': len(raw), 'maxbri': cfg.get('if', {}).get('live', {}).get('maxbri'),
                'scale_bri': cfg.get('light', {}).get('scale-bri')}
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
    return (f'40 % after {evidence["note_40_seconds"]} s, gone {evidence["force_max_seconds"]} s '
            'after Force max'), evidence


def dither(ctx):
    prysm = _prysm(ctx)
    _start_processed(ctx)
    found = {}
    for value in (False, True):
        reply = prysm.change_settings(lambda s: s['output'].__setitem__('dither', value))
        if reply.status != 200:
            raise Failed(f'dither {value}: HTTP {reply.status}', reply.json())
        state = prysm.wait(lambda s: prysm_api.running(s) and s['settings']['output'].get('dither') == value, 15)
        if state is None:
            raise Failed(f'not running again with dither {value}', prysm.state())
        found[str(value)] = rates(prysm.sample(3))
    return 'accepted off and on, lighting kept running', found


def release(ctx):
    prysm = _prysm(ctx)
    off(ctx)
    state = _start_processed(ctx, seconds=20)
    measured = rates(prysm.sample(ctx.config.desktop.seconds))
    if measured['fps_mean'] <= 0:
        raise Failed('no frames after re-opening', measured)
    _feed_settled(ctx)
    return f're-opened: {measured["fps_mean"]} fps', {'first': state['input'], 'rates': measured}


def display_check(ctx):
    if ctx.options.source != 'device':
        raise Skipped('needs --source device with this Mac as the card\'s input')
    page = colour_page.ColourPage(ctx.config.desktop.display_port).start()
    try:
        answer, note = ctx.ask(f'Open {page.url} in a browser on the display the capture card records '
                               '(this Mac\'s HDMI output), press F for full screen. Ready?', ('y', 'skip'))
        if answer != 'y':
            raise Skipped(f'operator skipped the display set-up {note}'.strip())
        _start_processed(ctx)
        results = {}
        ok = True
        for name, edges in colour_page.FIELDS:
            if not page.show(name, edges):
                raise Failed(f'the page did not show {name} within 10 s (is it open?)', results)
            time.sleep(1.5)
            event = _last_colours(ctx, 2.0)
            matched, means = colour_page.matches(event, edges)
            results[name] = {'ok': matched, 'measured': means, 'expected': edges,
                             'full_screen': (page.shown or {}).get('full')}
            ok = ok and matched
        page.show('', None, 2)
        if not ok:
            raise Failed('some fields differ beyond the tolerance', results)
        return f'{len(results)} fields, each edge the expected colour', results
    finally:
        page.close()


def sigterm(ctx):
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


def _feed_step(controlled, options):
    """The setup step that puts the clip into the capture card, or a skip that says why nothing does."""
    if controlled:
        return Step('desktop.feed', 'Picture into the capture card', controlled.how(), 'setup',
                    lambda ctx: show_clip(ctx, controlled), actions=controlled.actions(),
                    commands=controlled.commands)
    why = ('--display-check: the card records this computer\'s display, so no feed is started' if options.display_check
           else 'no feed in the configuration ([feed]): the card records whatever is plugged into it')
    return Step('desktop.feed', 'Picture into the capture card', why + '; the colour check is only "not black"',
                'skip')


def steps(config, options):
    device = options.source == 'device'
    controlled = _feed(config, options)
    found = [
        Step('desktop.preflight', 'Configuration and tools', 'WLED address and LEDs set; FFmpeg for the test clip; '
             'the feed\'s tools', 'setup', preflight),
        Step('desktop.build', 'Release prysm-headless', 'cargo build --release (or the configured binary)', 'setup',
             build, commands=lambda: [] if config.desktop.headless or options.no_build
             else [commands.cargo_build_headless()]),
        Step('desktop.wled-snapshot', 'WLED snapshot', 'GET /json/si and /json/cfg saved before anything changes; '
             'restored at the end, also on failure or Ctrl-C', 'setup', snapshot_wled,
             actions=[f'GET http://{config.wled}/json/si', f'GET http://{config.wled}/json/cfg']),
    ]
    if not device or controlled:
        found.append(Step('desktop.clip', 'Test clip', 'a generated clip with known edge colours, or --source file:PATH',
                          'setup', clip, commands=lambda: [] if options.source_path else [commands.ffmpeg_edges_clip(
                              'RUN_DIR/edges.mp4', CLIP['width'], CLIP['height'], CLIP['fps'], CLIP['seconds'],
                              CLIP_EDGES)]))
    if device:
        found.append(_feed_step(controlled, options))
    found += [
        Step('desktop.settings', 'Temporary settings', 'WLED output, neutral processing; --check-settings says OK',
             'setup', write_settings,
             commands=lambda: [commands.check_settings(host.binary(config), 'RUN_DIR/settings.json')]),
        Step('desktop.start', 'Start prysm-headless', 'GET /api/v1/state answers', 'setup', start,
             commands=lambda: [commands.headless(host.binary(config), 'RUN_DIR/settings.json', config.desktop.listen)]),
        Step('desktop.source', 'Video source', 'device: candidates, modes and the resolved preset printed; '
             'PUT /api/v1/settings with the capture', 'setup', source,
             actions=['GET /api/v1/candidates/capture/device', 'POST /api/v1/capture/modes',
                      'PUT /api/v1/settings'] if device else ['PUT /api/v1/settings (capture file, repeat)']),
        Step('desktop.processed', 'Screen sync starts', 'PUT /api/v1/target processed is 204; session running, '
             'frames arriving within 15 s', 'auto', processed),
        Step('desktop.wled-live', 'WLED shows Prysm', 'WLED /json/si: live, lm DDP, lip this computer; '
             'no output_issue', 'auto', wled_live),
        Step('desktop.fps', 'Frame rate', f'state.input over {config.desktop.seconds} s: fps within '
             f'±{config.desktop.fps_tolerance:.0%} of the source, sent_fps alike, never behind', 'auto', fps),
        Step('desktop.colours', 'Colours', 'colours stream: each edge (middle half) is nearest the generated clip\'s colour '
             'among nine named colours (a capture card without a feed: not black)', 'auto', colours),
        Step('desktop.device-native', 'WLED effects', 'target device_native: session native; WLED on, not live',
             'auto', device_native),
        Step('desktop.processed-again', 'Screen sync again', 'target processed: running and WLED live DDP',
             'auto', processed_again),
        Step('desktop.off', 'Lights off', 'target off: session stopped; WLED on=false', 'auto', off),
        Step('desktop.brightness', 'WLED brightness', 'bri 102: output_brightness 40 % within 5 s; '
             'if.live.maxbri on: back to 100 % within 35 s, state.bri unchanged; /json/cfg under 64 KiB '
             '(WLED changes restored)', 'auto', brightness,
             actions=['POST /json/state {"bri":102}', 'POST /json/cfg {"if":{"live":{"maxbri":true}}}',
                      'restore maxbri and bri']),
        Step('desktop.gamma', 'WLED gamma', 'Prysm does not expose the gamma it reads; judge by eye '
             '(stage eyes: dark scenes) or with the output-wled unit tests', 'skip'),
        Step('desktop.dither', 'Smooth dark colours switch', 'output.dither false then true: 200, '
             'lighting running again each time', 'auto', dither),
    ]
    if device:
        found.append(Step('desktop.release', 'Capture release', 'off, then processed: the device opens again '
                          'with frames', 'auto', release))
        if options.display_check:
            found.append(Step('desktop.display-check', 'Colour fields on the capture display',
                              'a local page shows red, green, blue, white and an edge pattern full screen; each '
                              'edge (middle half) is nearest the field\'s colour among nine named colours', 'guided', display_check,
                              actions=[f'serve http://127.0.0.1:{config.desktop.display_port}/ (localhost only)']))
    found.append(Step('desktop.sigterm', 'Clean exit', 'SIGTERM while lighting: exit code 0; WLED released',
                      'auto', sigterm))
    return found
