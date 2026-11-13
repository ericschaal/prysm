#!/usr/bin/env python3
"""Native smoke test of a v1 TV supervisor under an isolated probe identity.

Runs the cargo binaries from a webOS build (build-ipk.sh, or cargo with
env.sh and --profile webos) on a rooted TV over SSH, never the installed
package: everything lives in a private /tmp/prysm-supervisor.* directory, the
Luna name is io.prysm.tv.probe.service, the service runs with `run --sandbox
DIR --run-for N`, and the only file outside that directory is one probe
manifest, removed afterwards only if it is still exactly ours.

  basic (default)  Luna activation, power observation, public bus, malformed
                   request, subscription, sender check, SIGTERM, reactivation
                   and the bounded --run-for exit.
  --supervisor     also sandboxed `setup` (stub rescan, real activation),
                   app-only mutations, a fake worker that ignores SIGTERM
                   (TERM, KILL after 8 s, reap) and `uninstall`.
  --capture        the real worker with its worker role instead of the fake:
                   full serves the portal; slim streams to --prysm.

--dry-run writes nothing remotely: it prints the probe files and the plan.
Refuses --capture while another Prysm process runs on the TV (the old
io.prysm.capture app included): stop it first; this script never does.
"""
import argparse
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parents[1]
V1 = HERE.parents[1]
APP = 'io.prysm.tv.probe'
SERVICE = APP + '.service'
MANIFEST = f'/var/luna-service2/manifests.d/{APP}.json'
INSTALLED_SERVICE = 'io.prysm.tv.service'
TARGET = 'armv7-unknown-linux-gnueabi/webos'
# Cargo binaries (supervisor, worker) per flavour: identity.rs build_names.
BINARIES = {
    'full': ('prysm-webos-service', 'prysm-webos'),
    'slim': ('prysm-webos-slim-service', 'prysm-webos-slim'),
}
STATUS = json.dumps({'type': 'status', 'capturing': False, 'target': None, 'portal': None, 'failure': None})


def fake_worker(ignore_term):
    """Speaks the worker protocol (a status line every second); one that ignores
    SIGTERM makes the supervisor escalate to SIGKILL."""
    trap = "trap '' TERM\n" if ignore_term else ''
    return f"#!/bin/sh\n{trap}while :; do printf '%s\\n' {shlex.quote(STATUS)}; sleep 1; done\n"


DIRECTORY = re.compile(r'^/tmp/prysm-supervisor\.[A-Za-z0-9]{6,}$')


def template(name):
    return json.loads((HERE / 'service' / name).read_text())


def probe_files(directory, flavour, real_worker, run_for):
    """The probe's Luna files, derived from the packaged ones with probe names and paths."""
    supervisor, worker = BINARIES[flavour]
    role = template('supervisor.role.json')
    role['exeName'] = f'{directory}/{supervisor}'
    role['allowedNames'] = [SERVICE]
    for permission in role['permissions']:
        permission['service'] = SERVICE
    client = {SERVICE: template('supervisor.client.json')[INSTALLED_SERVICE]}
    api = {'public': [SERVICE + method[len(INSTALLED_SERVICE):]
                      for method in template('supervisor.api.json')['public']]}
    files = {
        'probe.role.json': json.dumps(role),
        'probe.client.json': json.dumps(client),
        'probe.api.json': json.dumps(api),
        'probe.service': (f'[D-BUS Service]\nName={SERVICE}\n'
                          f'Exec={directory}/{supervisor} run --sandbox {directory} --run-for {run_for}\n'
                          'Type=dynamic\n'),
    }
    manifest = {
        'id': APP, 'version': '1.0.0',
        'roleFiles': [f'{directory}/probe.role.json'],
        'serviceFiles': [f'{directory}/probe.service'],
        'clientPermissionFiles': [f'{directory}/probe.client.json'],
        'apiPermissionFiles': [f'{directory}/probe.api.json'],
    }
    if real_worker:
        worker_role = template('worker.role.json')
        worker_role['exeName'] = f'{directory}/{worker}'
        files['probe.worker.role.json'] = json.dumps(worker_role)
        manifest['roleFiles'].append(f'{directory}/probe.worker.role.json')
    return files, (json.dumps(manifest) + '\n').encode()


class Remote:
    def __init__(self, host):
        self.ssh = ['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8', host]

    def run(self, command, data=None, timeout=15, check=True):
        return subprocess.run(self.ssh + [command], input=data, capture_output=True,
                              timeout=timeout, check=check)

    def text(self, command, **options):
        return self.run(command, **options).stdout.decode().strip()

    def write(self, path, data, mode='600'):
        # noclobber: an existing path, including a dangling link, is a hard failure.
        if isinstance(data, str):
            data = data.encode()
        self.run(f'set -euC; umask 077; cat > {shlex.quote(path)}; chmod {mode} {shlex.quote(path)}', data)

    def luna(self, method, payload, app=None, public=False):
        binary = 'luna-send-pub' if public else 'luna-send'
        identity = ' -a ' + shlex.quote(app) if app else ''
        command = (f'timeout 12 {binary}{identity} -w 10000 -n 1 '
                   f'luna://{SERVICE}/{method} {shlex.quote(json.dumps(payload))}')
        # This TV's luna-send exits without a reply when stdin reaches EOF.
        with subprocess.Popen(self.ssh + [command], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
            try:
                output, error = process.communicate(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.communicate()
                raise
        if process.returncode != 0 or not output:
            raise RuntimeError(f'{method} failed ({process.returncode}): {output!r} {error!r}')
        return json.loads(output)

    def pids(self, executable):
        """Processes whose executable is exactly `executable`."""
        output = self.text(
            'for process in /proc/[0-9]*/exe; do '
            f'if [ "$(readlink "$process")" = {shlex.quote(executable)} ]; then '
            'pid=${process#/proc/}; echo "${pid%/exe}"; fi; done', check=False)
        return [int(pid) for pid in output.split()]


def eventually(probe, check, label, seconds=10):
    deadline = time.monotonic() + seconds
    value = None
    while time.monotonic() < deadline:
        value = probe()
        if check(value):
            return value
        time.sleep(0.3)
    raise AssertionError(f'{label}: {value}')


def passed(message):
    print('PASS ' + message, flush=True)


def smoke(args, remote):
    flavour = args.flavour
    supervisor_name, worker_name = BINARIES[flavour]
    artifacts = args.build_dir / TARGET
    real_worker = args.capture
    if args.capture:
        running = remote.text('ps -ef')
        assert not re.search(r'prysm-webos|prysm-capture', running), \
            'a Prysm process is running on the TV; stop it before --capture'
    directory = remote.text('mktemp -d /tmp/prysm-supervisor.XXXXXX')
    if not DIRECTORY.match(directory):
        raise RuntimeError('unexpected temporary path: ' + directory)
    supervisor = f'{directory}/{supervisor_name}'
    worker = f'{directory}/{worker_name}'
    files, manifest = probe_files(directory, flavour, real_worker, args.run_for)
    installed = False

    def state(**options):
        return remote.luna('getState', {}, **options)

    try:
        remote.write(supervisor, (artifacts / supervisor_name).read_bytes(), '700')
        if real_worker:
            remote.write(worker, (artifacts / worker_name).read_bytes(), '700')
        else:
            # Without --supervisor the fake honours SIGTERM, so shutdown stays quick.
            remote.write(worker, fake_worker(ignore_term=args.supervisor), '700')
        for name, content in files.items():
            remote.write(f'{directory}/{name}', content)
        if args.supervisor:
            # Sandboxed setup: manifests and hook stay in the directory; the
            # rescan is a stub, activation goes through the real bus.
            remote.run(f'mkdir -m 700 {directory}/manifests.d')
            remote.write(f'{directory}/ls-control', '#!/bin/sh\nexit 0\n', '700')
            remote.write(f'{directory}/luna-send', '#!/bin/sh\nexec /usr/bin/luna-send "$@"\n', '700')
        remote.write(MANIFEST, manifest, '644')
        installed = True
        remote.run('ls-control scan-services', timeout=20)

        if args.supervisor:
            output = remote.text(f'{supervisor} setup --sandbox {directory}', timeout=30)
            assert f'{SERVICE} is registered and running' in output, output
            for path in (f'manifests.d/{APP}.supervisor.json', f'manifests.d/{APP}.worker.json', f'init.d/{APP}'):
                remote.run(f'test -f {directory}/{path}')
            passed('sandboxed setup wrote manifests and hook and activated the service')

        value = state()  # Activated through Luna, not started over SSH.
        assert value['returnValue'] is True and value['flavor'] == flavour, value
        value = eventually(state, lambda v: v['power'] != 'unknown', 'power observation')
        passed('Luna activation and power observation: ' + json.dumps(value))
        (pid,) = remote.pids(supervisor)
        assert state(public=True)['returnValue'] is True
        passed('public-bus getState')
        assert remote.luna('getState', {'subscribe': 'yes'})['returnValue'] is False
        passed('malformed request rejected')
        assert remote.luna('getState', {'subscribe': True})['subscribed'] is True
        passed('subscription acknowledged')
        for method, payload in (('setEnabled', {'enabled': True}), ('setPreferences', {'autostart': False}),
                                ('uninstall', {})):
            assert remote.luna(method, payload)['returnValue'] is False, method
        passed('mutations refused without the app identity')

        if args.supervisor:
            exercise_worker(args, remote, state, directory, worker)
            started = time.monotonic()
            assert remote.luna('uninstall', {}, app=APP)['returnValue'] is True
            remote.run(f'timeout 20 sh -c "while kill -0 {pid} 2>/dev/null; do sleep 0.2; done"', timeout=25)
            elapsed = time.monotonic() - started
            assert not remote.pids(worker), 'worker left running'
            if not real_worker:
                assert 7 <= elapsed <= 15, f'TERM-to-KILL took {elapsed:.1f} s'
            for path in (f'manifests.d/{APP}.supervisor.json', f'manifests.d/{APP}.worker.json', f'init.d/{APP}'):
                remote.run(f'test ! -e {directory}/{path}')
            passed(f'uninstall stopped the worker, removed registration and exited ({elapsed:.1f} s)')
        else:
            remote.run(f'kill -TERM {pid}')
            remote.run(f'timeout 12 sh -c "while kill -0 {pid} 2>/dev/null; do sleep 0.1; done"')
            assert state()['returnValue'] is True
            (restarted,) = remote.pids(supervisor)
            assert restarted != pid
            passed('SIGTERM shutdown and Luna reactivation')
            print('Probe', remote.text(f"sed -n '/^VmRSS:/p' /proc/{restarted}/status"), flush=True)
            remote.run(f'timeout {args.run_for + 6} sh -c "while kill -0 {restarted} 2>/dev/null; do sleep 0.2; done"',
                       timeout=args.run_for + 10)
            passed('bounded --run-for exit')
    finally:
        cleanup(remote, directory, supervisor, worker, manifest if installed else None)


def exercise_worker(args, remote, state, directory, worker):
    """Start a worker through the app-only API and watch it run."""
    app = {'app': APP}
    if args.flavour == 'slim':
        prysm = args.prysm if args.capture else 'http://127.0.0.1:9'
        value = remote.luna('setPreferences', {'autostart': False, 'instance': prysm}, **app)
        assert value['returnValue'] is True and value['preferences']['instance'] == prysm, value
        passed('setPreferences persisted')
        remote.run(f'test -f {directory}/state/tv.json')
    value = remote.luna('setEnabled', {'enabled': True}, **app)
    assert value['returnValue'] is True, value
    value = eventually(state, lambda v: v['worker'] == 'running', 'worker running (is the TV on?)', 15)
    assert remote.pids(worker), 'no worker process'
    passed('worker spawned: ' + json.dumps(value))
    if not args.capture:
        return
    if args.flavour == 'full':
        value = eventually(state, lambda v: v['portal'], 'portal address', 15)
        passed('full worker serves the portal at ' + value['portal'])
        time.sleep(5)
        print('Full worker state after enabling:', json.dumps(state()), flush=True)
    else:
        value = eventually(state, lambda v: v['capturing'], 'frames to ' + args.prysm, 20)
        passed('slim worker streams to ' + args.prysm)


def cleanup(remote, directory, supervisor, worker, manifest):
    for executable in (supervisor, worker):
        for pid in remote.pids(executable):
            remote.run(f'kill -KILL {pid}', check=False)
    if manifest is not None:
        current = remote.run(f'cat {MANIFEST}', check=False)
        if current.returncode == 0 and current.stdout == manifest:
            remote.run(f'rm {MANIFEST}; ls-control scan-services', timeout=20)
        elif current.returncode == 0:
            raise RuntimeError(f'{MANIFEST} changed; leaving {directory} for inspection')
    if DIRECTORY.match(directory):
        remote.run(f'rm -rf -- {shlex.quote(directory)}')
    print('Removed the probe registration and files.', flush=True)


def dry_run(args):
    directory = '/tmp/prysm-supervisor.XXXXXX'
    files, manifest = probe_files(directory, args.flavour, args.capture, args.run_for)
    supervisor, worker = BINARIES[args.flavour]
    for name in (supervisor,) + ((worker,) if args.capture else ()):
        path = args.build_dir / TARGET / name
        print(f'binary {path}: {"present" if path.is_file() else "MISSING (build first)"}')
    print(f'--- {MANIFEST}\n{manifest.decode()}', end='')
    for name, content in files.items():
        print(f'--- {directory}/{name}\n{content}' + ('' if content.endswith('\n') else '\n'), end='')
    if not args.capture:
        print(f'--- {directory}/{worker} (fake)\n{fake_worker(args.supervisor)}', end='')
    print('Dry run: nothing was sent to a TV.')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--host', help='SSH destination, e.g. root@192.168.1.10')
    parser.add_argument('--flavour', choices=sorted(BINARIES), default='full')
    parser.add_argument('--supervisor', action='store_true', help='setup, app-only API, fake worker, uninstall')
    parser.add_argument('--capture', action='store_true', help='the real worker (implies --supervisor)')
    parser.add_argument('--prysm', help='slim --capture: a running Prysm with the remote source and lighting on, http://HOST:8787')
    parser.add_argument('--build-dir', type=Path, default=V1 / 'target', help='Cargo target directory')
    parser.add_argument('--run-for', type=int, help='probe lifetime in seconds, 10 to 300 (30; 120 with --supervisor)')
    parser.add_argument('--dry-run', action='store_true', help='print the probe files; touch no TV')
    args = parser.parse_args()
    args.supervisor = args.supervisor or args.capture
    if args.run_for is None:
        args.run_for = 120 if args.supervisor else 30
    if not 10 <= args.run_for <= 300:
        parser.error('--run-for must be 10 to 300 seconds')
    if args.capture and args.flavour == 'slim' and not args.prysm:
        parser.error('slim --capture needs --prysm')
    if args.dry_run:
        dry_run(args)
        return 0
    if not args.host:
        parser.error('--host is required without --dry-run')
    smoke(args, Remote(args.host))
    return 0


if __name__ == '__main__':
    sys.exit(main())
