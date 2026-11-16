"""The slim package's checks: the TV streams to a receiver (a prysm-headless here, or --receiver URL)."""
import time

import commands
import host
from plan import Failed, Skipped
import prysm_api
from prysm_api import near, rates
import stream_version
import tv

ENCODINGS = ('h264_hardware', 'h264_software', 'raw')
SOAK = (640, 360, 30, 'h264_hardware')


def receiver(ctx):
    return ctx.data['receiver']


def start_receiver(ctx):
    config = ctx.config
    if config.receiver.url:
        prysm = prysm_api.Prysm(config.receiver.url)
        saved = prysm.state()
        ctx.save('receiver-as-found.json', saved)

        def restore_receiver():
            reply = prysm.replace_settings(saved['settings'])
            target = prysm.set_target(saved['target'])
            if reply.status != 200 or target.status != 204:
                raise RuntimeError(f'receiver not restored: settings HTTP {reply.status}, target HTTP {target.status}')
            return f'settings and target {saved["target"]} restored'
        ctx.cleanup('tv.restore-receiver', 'Receiver settings and target restored', restore_receiver)
        ctx.data['receiver'] = prysm
        ctx.data['receiver_for_tv'] = config.receiver.url
        return f'using {config.receiver.url}'
    document = host.settings(host.wled_output(config), {'kind': 'remote'})
    path = host.write_settings(ctx, 'receiver-settings.json', document)
    local = host.Headless(ctx, str(path), config.receiver.listen, allow_null_origin=True, name='receiver')
    ctx.cleanup('tv.stop-receiver', 'Local receiver stopped', lambda: f'exit code {local.stop()}')
    local.start()
    ctx.data['receiver'] = prysm_api.Prysm(local.url)
    address = config.receiver.advertise or host.local_address(config.tv.address)
    port = config.receiver.listen.rsplit(':', 1)[1]
    ctx.data['receiver_for_tv'] = f'http://{address}:{port}'
    return f'prysm-headless receiving at {ctx.data["receiver_for_tv"]}'


def point_tv_at(ctx, instance):
    tv.call(ctx, commands.tv_set_preferences(ctx.config.tv.ssh, False, instance))


def connect(ctx):
    receiver(ctx).set_target('off')
    point_tv_at(ctx, ctx.data['receiver_for_tv'])
    tv.call(ctx, commands.tv_set_enabled(ctx.config.tv.ssh, True))
    return f'instance {ctx.data["receiver_for_tv"]}, enabled'


def request(prysm, width, height, fps, encoding):
    """Save the remote capture for this stream and turn lighting on; the reply."""
    capture = {'kind': 'remote', 'width': width, 'height': height, 'fps': fps, 'include_ui': True,
               'encoding': encoding}
    reply = prysm.change_settings(lambda s: s.__setitem__('capture', capture))
    if reply.status != 200:
        return reply
    return prysm.set_target('processed')


def stream(ctx, width=640, height=360, fps=30, encoding='h264_hardware', seconds=30):
    """Lighting on at the receiver with this stream, and frames arriving."""
    prysm = receiver(ctx)
    reply = request(prysm, width, height, fps, encoding)
    if reply.status != 204:
        raise Failed(f'{encoding} {width}x{height}@{fps}: HTTP {reply.status}', reply.json())
    time.sleep(2)
    state = prysm.wait(prysm_api.running, seconds)
    if state is None:
        raise Failed(f'no frames from the TV within {seconds} s ({encoding})', prysm.state())
    return state


def offer_while_off(ctx):
    prysm = receiver(ctx)
    found = {}
    deadline = time.monotonic() + 30
    for encoding in ENCODINGS:
        while True:
            answer = prysm.modes({'kind': 'remote', 'encoding': encoding}).json()
            if answer.get('status') == 'known' or time.monotonic() > deadline:
                break
            time.sleep(1)
        found[encoding] = answer
    unknown = [e for e, a in found.items() if a.get('status') != 'known']
    if unknown:
        raise Failed(f'no offer known with lights off for {unknown}', found)
    if prysm.state()['target'] != 'off':
        raise Failed('the receiver is not off', found)
    hardware = found['h264_hardware'].get('modes') or []
    found['hardware_max_fps'] = max((m['fps']['max'] for m in hardware), default=None)
    return '; '.join(f'{e}: {len(found[e]["modes"])} modes' for e in ENCODINGS), found


def encodings(ctx):
    prysm = receiver(ctx)
    results, failed = {}, []
    for encoding in ENCODINGS:
        try:
            stream(ctx, encoding=encoding)
        except Failed as error:
            results[encoding] = {'failed': str(error), 'evidence': error.evidence}
            failed.append(encoding)
            continue
        first = tv.sample_worker(ctx)
        measured = rates(prysm.sample(ctx.config.desktop.seconds))
        measured['tv_cpu'] = tv.cpu_percent(first, tv.sample_worker(ctx))
        measured['tv_encoding'] = (tv.get_state(ctx) or {}).get('encoding')
        results[encoding] = measured
        if not near(measured['fps_mean'], 30, ctx.config.desktop.fps_tolerance) or measured['tv_encoding'] != encoding:
            failed.append(encoding)
    prysm.set_target('off')
    if failed:
        raise Failed(f'not 30 fps (or another encoding) for {failed}', results)
    return ', '.join(f'{e} {r["fps_mean"]} fps (TV CPU {r["tv_cpu"]} %)' for e, r in results.items()), results


def soak(ctx):
    width, height, fps, encoding = SOAK
    stream(ctx, width, height, fps, encoding)
    result = tv.soak(ctx, receiver(ctx), ctx.config.tv.soak_seconds)
    result['stream'] = f'{encoding} {width}x{height}@{fps}'
    if not near(result['fps_mean'], fps, ctx.config.desktop.fps_tolerance):
        raise Failed(f'{result["fps_mean"]} fps over the soak, expected {fps}', result)
    return tv.soak_summary(result), result


def hardware_60_refused(ctx):
    prysm = receiver(ctx)
    reply = request(prysm, 640, 360, 60, 'h264_hardware')
    body = reply.json() if reply.body else None
    evidence = {'status': reply.status, 'reply': body, 'state_failure': prysm.state().get('failure')}
    prysm.set_target('off')
    failure = body if reply.status != 204 else evidence['state_failure']
    if not failure or failure.get('field') not in ('capture.fps', 'capture.encoding'):
        raise Failed('60 fps with hardware H.264 was not refused on capture.fps', evidence)
    return f'refused: {failure.get("message")}', evidence


def pause_resume(ctx):
    prysm = receiver(ctx)
    stream(ctx)
    prysm.set_target('off')
    start = time.monotonic()
    paused = tv.wait_state(ctx, lambda s: s['paused'] and not s['capturing'], 15)
    paused_after = round(time.monotonic() - start, 1)
    prysm.set_target('processed')
    start = time.monotonic()
    resumed = tv.wait_state(ctx, lambda s: s['capturing'] and not s['paused'], 20)
    evidence = {'paused': paused, 'paused_after_s': paused_after, 'resumed': resumed,
                'resumed_after_s': round(time.monotonic() - start, 1)}
    if paused is None or resumed is None:
        raise Failed('the TV did not pause and resume', evidence)
    return f'paused after {paused_after} s, resumed after {evidence["resumed_after_s"]} s', evidence


def version_mismatch(ctx):
    status, protocol = stream_version.ask(receiver(ctx).base, stream_version.OTHER)
    evidence = {'receiver_answer': {'status': status, 'protocol': protocol}}
    if status != 426 or protocol != stream_version.CURRENT:
        raise Failed('the receiver did not answer 426 naming its version', evidence)
    stand_in = stream_version.OtherVersion(ctx.config.receiver.mismatch_port).start()
    address = ctx.config.receiver.advertise or host.local_address(ctx.config.tv.address)
    try:
        point_tv_at(ctx, f'http://{address}:{stand_in.port}')
        state = tv.wait_state(ctx, lambda s: (s.get('failure') or {}).get('kind') == 'version', 30)
    finally:
        point_tv_at(ctx, ctx.data['receiver_for_tv'])
        stand_in.close()
    evidence['tv_failure'] = state and state['failure']
    evidence['stand_in_requests'] = stand_in.requests
    if state is None:
        raise Failed('the TV did not report a version failure within 30 s', evidence)
    return f'receiver 426 {protocol}; TV says: {state["failure"]["message"][:160]}', evidence


def wss(ctx):
    url = ctx.config.receiver.wss_url
    if not url.startswith('https://'):
        raise Skipped('receiver.wss_url (or --wss-url) must be an https:// Prysm')
    try:
        point_tv_at(ctx, url)
        state = tv.wait_state(ctx, lambda s: s['capturing'] and (s.get('fps') or 0) > 0, 45)
        last = state or tv.get_state(ctx)
    finally:
        point_tv_at(ctx, ctx.data['receiver_for_tv'])
    if state is None:
        raise Failed('no frames over wss within 45 s (is lighting on at that Prysm?)', last)
    return f'streaming over wss at {state["fps"]} fps', state


def sweep(ctx):
    prysm = receiver(ctx)
    rows = []
    for encoding in ENCODINGS:
        for width, height, fps in ctx.config.sweep.pairs():
            rows.append({'flavour': 'slim', 'encoding': encoding, 'width': width, 'height': height,
                         'requested_fps': fps,
                         **tv.measure_row(ctx, prysm, lambda w=width, h=height, f=fps, e=encoding:
                                          request(prysm, w, h, f, e))})
            prysm.set_target('off')
    path = tv.write_table(ctx, 'sweep-slim.csv', rows)
    return f'{len(rows)} rows in {path}', rows
