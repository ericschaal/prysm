#!/usr/bin/env python3
"""Exercise the deployment image with empty storage, without capture hardware."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid


def docker(*args):
    return subprocess.check_output(
        ['docker', *args], stderr=subprocess.STDOUT, text=True, timeout=120,
    ).strip()


class Container:
    def __init__(self, image, mount, user=None, environment=(), arguments=()):
        self.name = 'prysm-smoke-' + uuid.uuid4().hex[:12]
        self.image, self.mount = image, mount
        self.options = []
        if user:
            self.options += ['--user', user]
        for value in environment:
            self.options += ['--env', value]
        self.arguments = arguments
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def __enter__(self):
        try:
            docker('run', '--detach', '--name', self.name, '--read-only',
                   '--cap-drop=ALL', '--security-opt=no-new-privileges',
                   '--mount', self.mount, '--publish', '127.0.0.1::8787',
                   *self.options, self.image, *self.arguments)
            self.base = 'http://' + docker('port', self.name, '8787/tcp')
            deadline = time.monotonic() + 30
            while True:
                try:
                    self.request('/api/settings')
                    return self
                except (urllib.error.URLError, ConnectionError, TimeoutError):
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.2)
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def __exit__(self, kind, value, traceback):
        try:
            if kind is None:
                docker('stop', '--time', '90', self.name)
                assert docker('inspect', '--format', '{{.State.ExitCode}}', self.name) == '0'
            else:
                subprocess.run(['docker', 'logs', self.name], check=False, timeout=10)
        finally:
            subprocess.run(['docker', 'rm', '--force', self.name], check=False,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=120)

    def request(self, path, changes=None, status=200, host='localhost:8787'):
        data = None if changes is None else json.dumps(changes).encode()
        request = urllib.request.Request(
            self.base + path, data=data, method='GET' if data is None else 'PUT',
            headers={'Host': host, 'Origin': 'http://' + host,
                     'Content-Type': 'application/json'},
        )
        try:
            response = self.opener.open(request, timeout=3)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            body = response.read()
            assert response.status == status, (response.status, body.decode())
            return json.loads(body) if path.startswith('/api/') else body

    def save(self, changes, status=200):
        view = self.request('/api/settings')
        settings = view['settings']
        for key, value in changes.items():
            if key == 'processing':
                settings[key].update(value)
            else:
                settings[key] = value
        return self.request('/api/settings', {
            'expected_revision': view['revision'], 'settings': settings,
        }, status=status)

    def saved(self):
        return docker('exec', self.name, 'cat', '/config/settings.json')


def configure(container, user):
    settings = container.request('/api/settings')
    assert set(settings['missing_required']) == {'input', 'output'}
    docker('exec', container.name, 'sh', '-c', 'test ! -e "$PRYSM_CONFIG"')
    assert docker('exec', container.name, 'id', '-u') == user
    html = container.request('/').decode()
    script = re.search(r'src="([^"]+\.js)"', html)
    assert script and len(container.request(script[1])) > 1000
    container.request('/api/settings', status=403, host='untrusted.example:8787')
    container.save({'output': {'type': 'wled', 'url': 'file:///tmp/invalid',
                               'leds': {'top': 1, 'right': 0, 'bottom': 0, 'left': 0}}}, status=422)
    assert container.request('/api/settings')['revision'] == settings['revision']
    configured = container.save({
        'input': {'type': 'webos',
                  'capture_resolution': [640, 360], 'refresh_rate_hz': 30},
        'output': {'type': 'wled', 'url': 'http://192.0.2.1/',
                   'leds': {'top': 51, 'right': 31, 'bottom': 49, 'left': 31}},
        'processing': {'brightness_percent': 79},
    })
    assert configured['missing_required'] == []
    assert configured['revision'] != settings['revision']
    assert configured['settings']['processing']['brightness_percent'] == 79
    container.request('/api/state')
    return configured


def main():
    image = sys.argv[1] if len(sys.argv) > 1 else 'prysm:local'
    volume = 'prysm-smoke-' + uuid.uuid4().hex[:12]
    docker('volume', 'create', volume)
    try:
        subprocess.run(['docker', 'run', '--rm', '--network=none', image, '--help'],
                       check=True, capture_output=True, text=True, timeout=30)
        missing = subprocess.run(['docker', 'run', '--rm', '--network=none', image, 'check'],
                                 capture_output=True, text=True, timeout=30)
        assert missing.returncode != 0, 'check must reject incomplete settings'
        assert 'settings are incomplete' in missing.stderr, missing.stderr
        mount = f'type=volume,src={volume},dst=/config'
        with Container(image, mount) as container:
            assert set(container.request('/api/settings')['missing_required']) == {'input', 'output'}
            docker('exec', container.name, 'sh', '-c', 'test ! -e "$PRYSM_CONFIG"')
        with Container(image, mount) as container:
            configure(container, '10001')
            assert 'web' not in json.loads(container.saved())
        docker('run', '--rm', '--read-only', '--network=none', '--mount', mount,
               image, 'check')
        with Container(image, mount, environment=['PRYSM_LISTEN=0.0.0.0:8787']) as container:
            settings = container.request('/api/settings')
            assert 'web' not in settings['settings']
            container.save({'web': {'listen': '127.0.0.1:9000'}}, status=422)
            container.save({'processing': {'brightness_percent': 78}})
            stored = json.loads(container.saved())
            assert 'web' not in stored and 'listen' not in stored['input']
        with Container(image, mount, environment=['PRYSM_LISTEN=127.0.0.1:8787'],
                       arguments=['--listen', '0.0.0.0:8787']) as container:
            settings = container.request('/api/settings')
            assert settings['settings']['processing']['brightness_percent'] == 78
            assert json.loads(container.saved()) == stored
    finally:
        docker('volume', 'rm', volume)

    # The test fixture grants write access to an arbitrary numeric user, like a dataset ACL.
    with tempfile.TemporaryDirectory(prefix='prysm-smoke-', dir='.') as directory:
        path = Path(directory).resolve()
        os.chmod(path, 0o777)
        mount = f'type=bind,src={path},dst=/config'
        with Container(image, mount, user='568:568') as container:
            configured = configure(container, '568')
            assert docker('exec', container.name, 'stat', '-c', '%u:%g',
                          '/config/settings.json') == '568:568'
        with Container(image, mount + ',readonly', user='568:568') as container:
            restored = container.request('/api/settings')
            assert restored['settings'] == configured['settings']
            previous = container.saved()
            container.save({'processing': {'brightness_percent': 77}}, status=500)
            assert container.saved() == previous
            assert container.request('/api/settings')['revision'] == restored['revision']
        (path / 'settings.json').unlink()
        (path / 'settings.json').write_text('{invalid json')
        invalid = subprocess.run(['docker', 'run', '--rm', '--network=none', '--read-only',
                                  '--user', '568:568', '--mount', mount, image, 'check'],
                                 capture_output=True, text=True, timeout=30)
        assert invalid.returncode != 0
        assert (path / 'settings.json').read_text() == '{invalid json'
    print('Image smoke test passed: setup, process options, persistence, numeric UID, failed writes, SIGTERM.')


if __name__ == '__main__':
    main()
