"""A prysm-headless process on this computer: settings, start, SIGTERM stop."""
import json
import signal
import socket
import subprocess
import time

import commands
import web

# Processing that keeps colours as captured, so measured colours can be compared.
NEUTRAL_PROCESSING = {
    'brightness_percent': 100,
    'style': {'kind': 'custom'},
    'custom_style': {
        'smoothing_seconds': 0, 'cut_response_percent': 100, 'brightness_ramp_seconds': 0,
        'spatial_reach_percent': 10, 'color_focus_percent': 0, 'saturation_boost_percent': 0,
        'dark_floor_percent': 0,
    },
    'remove_black_bars': False,
}


def binary(config):
    return config.desktop.headless or str(commands.ROOT / 'target/release/prysm-headless')


def settings(output, capture=None):
    document = {'output': output, 'processing': NEUTRAL_PROCESSING}
    if capture:
        document['capture'] = capture
    return document


def wled_output(config, dither=True):
    return {'kind': 'wled', 'address': config.wled, 'leds': config.leds.as_settings(), 'dither': dither}


def local_address(towards):
    """This computer's address on the route to `towards` (a UDP connect sends nothing)."""
    host = towards.split(':')[0]
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        probe.connect((socket.gethostbyname(host), 9))
        return probe.getsockname()[0]


class Headless:
    """One running prysm-headless; `stop` sends SIGTERM and returns the exit code."""

    def __init__(self, ctx, settings_path, listen, allow_null_origin=False, name='headless'):
        self.ctx = ctx
        self.command = commands.headless(binary(ctx.config), settings_path, listen,
                                         allow_null_origin=allow_null_origin)
        port = listen.rsplit(':', 1)[1]
        host = listen.rsplit(':', 1)[0]
        self.url = f'http://{"127.0.0.1" if host == "0.0.0.0" else host}:{port}'
        self.log_path = ctx.out_dir / f'{name}.log'
        self.process = None
        self.exit_code = None

    def start(self, seconds=15):
        log = open(self.log_path, 'ab')
        self.process = self.ctx.transport.spawn(self.command, stdout=log, stderr=subprocess.STDOUT)
        log.close()
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f'prysm-headless exited {self.process.returncode}; see {self.log_path}')
            try:
                return web.get_json(self.url + '/api/v1/state', timeout=1)
            except (web.Unreachable, RuntimeError):
                time.sleep(0.2)
        raise RuntimeError(f'prysm-headless did not answer within {seconds} s; see {self.log_path}')

    def stop(self, seconds=30):
        if self.process is None or self.exit_code is not None:
            return self.exit_code
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
        try:
            self.exit_code = self.process.wait(seconds)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            self.exit_code = 'killed after no exit within %d s' % seconds
        return self.exit_code


def write_settings(ctx, name, document):
    path = ctx.out_dir / name
    path.write_text(json.dumps(document, indent=2) + '\n')
    return path
