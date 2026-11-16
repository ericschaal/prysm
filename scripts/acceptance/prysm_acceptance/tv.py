"""The TV through the allow-listed commands: Luna state, install, files, worker load."""
import csv
import json
import re
import time

import commands
from plan import Failed
import prysm_api
from transport import CommandFailed

CLOCK_TICKS = 100  # USER_HZ on the TV's ARM kernel (assumed; see the report)


def luna_reply(result):
    """The first JSON object luna-send printed."""
    for line in result.stdout.splitlines():
        line = line.strip()
        if line.startswith('{'):
            try:
                return json.loads(line)
            except ValueError:
                continue
    raise Failed('no JSON reply from luna-send', (result.stdout + result.stderr)[-1000:])


def get_state(ctx):
    """`getState`, or `None` when the service does not answer (not registered)."""
    result = ctx.run(commands.tv_get_state(ctx.config.tv.ssh), check=False)
    try:
        reply = luna_reply(result)
    except Failed:
        return None
    return reply if reply.get('returnValue') else None


def wait_state(ctx, check, seconds, every=2.0):
    deadline = time.monotonic() + seconds
    while True:
        state = get_state(ctx)
        if state is not None and check(state):
            return state
        if time.monotonic() >= deadline:
            return None
        time.sleep(every)


def call(ctx, command):
    reply = luna_reply(ctx.run(command, check=False))
    if not reply.get('returnValue'):
        raise Failed(f'{command.template} refused: {reply.get("errorText")}', reply)
    return reply


def paths(ctx):
    found = {}
    for line in ctx.run(commands.tv_paths(ctx.config.tv.ssh)).stdout.splitlines():
        kind, _, path = line.partition(' ')
        if kind in ('present', 'absent'):
            found[path] = kind == 'present'
    return found


def processes(ctx):
    return [line.split(' ', 1) for line in ctx.run(commands.tv_processes(ctx.config.tv.ssh)).stdout.splitlines()
            if line.strip()]


def read_data(ctx, name):
    text = ctx.run(commands.tv_read_data(ctx.config.tv.ssh, name)).stdout
    return None if text.strip() == '__absent__' else text


def install(ctx, ipk):
    """The documented install: stage, appInstallService, unstage, setup.sh."""
    host = ctx.config.tv.ssh
    ctx.run(commands.tv_stage(host, ipk))
    try:
        result = ctx.run(commands.tv_install(host), check=False)
    finally:
        ctx.run(commands.tv_unstage(host), check=False)
    if not result.ok:
        raise Failed('appInstallService did not report installed', (result.stdout + result.stderr)[-2000:])
    ctx.run(commands.tv_setup(host))
    state = wait_state(ctx, lambda s: True, 30)
    if state is None:
        raise Failed('the service does not answer getState 30 s after setup')
    return state


def sample_worker(ctx):
    """One reading: uptime, load average, the worker's CPU ticks and RSS (None without a worker)."""
    text = ctx.run(commands.tv_sample_worker(ctx.config.tv.ssh)).stdout
    lines = text.splitlines()
    reading = {'uptime': float(lines[0].split()[0]), 'load': lines[1].split()[0], 'ticks': None, 'rss_kb': None}
    for line in lines[2:]:
        if line.startswith('stat '):
            fields = line.rsplit(')', 1)[1].split()
            reading['ticks'] = int(fields[11]) + int(fields[12])  # utime + stime
        match = re.match(r'VmRSS:\s+(\d+)', line)
        if match:
            reading['rss_kb'] = int(match[1])
    return reading


def cpu_percent(first, second):
    """The worker's CPU between two readings, in percent of one core."""
    if first['ticks'] is None or second['ticks'] is None or second['uptime'] <= first['uptime']:
        return None
    return round(100 * (second['ticks'] - first['ticks']) / CLOCK_TICKS / (second['uptime'] - first['uptime']), 1)


def load_summary(readings):
    cpu = [c for c in (cpu_percent(a, b) for a, b in zip(readings, readings[1:])) if c is not None]
    rss = [r['rss_kb'] for r in readings if r['rss_kb']]
    return {'cpu_mean': round(sum(cpu) / len(cpu), 1) if cpu else None, 'cpu_max': max(cpu, default=None),
            'rss_max_kb': max(rss, default=None), 'load': [r['load'] for r in readings][-1:]}


def soak(ctx, prysm, seconds):
    """`prysm`'s input every second and the TV worker's load every 5 s, for `seconds`."""
    inputs, readings = [], []
    deadline = time.monotonic() + seconds
    next_reading = 0.0
    while time.monotonic() < deadline:
        if time.monotonic() >= next_reading:
            readings.append(sample_worker(ctx))
            next_reading = time.monotonic() + 5
        inputs.append(prysm.state().get('input'))
        time.sleep(1)
    readings.append(sample_worker(ctx))
    return {**prysm_api.rates(inputs), **load_summary(readings), 'seconds': seconds}


def soak_summary(result):
    return (f'{result["fps_mean"]} fps (min {result["fps_min"]}), worker CPU {result["cpu_mean"]} % '
            f'(max {result["cpu_max"]}), RSS {result["rss_max_kb"]} kB')


def measure_row(ctx, prysm, begin):
    """One row of a sweep: `begin()` asks for a size and rate; what `prysm` then receives."""
    reply = begin()
    if reply.status not in (200, 204):
        body = reply.json() or {}
        return {'measured_fps': '', 'sent_fps': '', 'result': f'refused: {body.get("message", reply.status)}'}
    time.sleep(2)  # the session restarts with the new request
    state = prysm.wait(prysm_api.running, 20)
    if state is None:
        failure = (prysm.state().get('failure') or {}).get('message', 'no frames')
        return {'measured_fps': '', 'sent_fps': '', 'result': f'not running: {failure}'}
    first = sample_worker(ctx)
    measured = prysm_api.rates(prysm.sample(ctx.config.sweep.seconds))
    return {'measured_fps': measured['fps_mean'], 'sent_fps': measured['sent_mean'],
            'tv_cpu': cpu_percent(first, sample_worker(ctx)), 'result': 'ok'}


def write_table(ctx, name, rows):
    path = ctx.out_dir / name
    fields = ['flavour', 'encoding', 'width', 'height', 'requested_fps', 'measured_fps', 'sent_fps', 'tv_cpu', 'result']
    with open(path, 'w', newline='') as file:
        writer = csv.DictWriter(file, fields)
        writer.writeheader()
        writer.writerows(rows)
    return str(path)


def remove_app(ctx):
    """ares-remove when a device name is configured; otherwise a person removes it."""
    if ctx.config.tv.ares_device:
        try:
            ctx.run(commands.ares_remove(ctx.config.tv.ares_device))
            return 'ares-remove'
        except CommandFailed as error:
            raise Failed(f'ares-remove failed: {error}')
    answer, note = ctx.ask('Uninstall the Prysm app on the TV now (launcher: long-press Prysm, Remove). Done?',
                           ('y', 'n'))
    if answer != 'y':
        raise Failed(f'the app was not removed {note}'.strip())
    return 'removed by hand'
