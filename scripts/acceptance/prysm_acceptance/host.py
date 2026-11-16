"""A prysm-headless process on this computer: build, settings, start, SIGTERM stop."""

import json
from pathlib import Path
import signal
import socket
import subprocess
import time
from typing import cast

from prysm_acceptance import commands, web
from prysm_acceptance.config import Config
from prysm_acceptance.plan import Bench, Failed
from prysm_acceptance.prysm_api import Settings, State
from prysm_acceptance.transport import Process
from prysm_acceptance.web import Json, JsonObject

# Processing that keeps colours as captured, so measured colours can be compared.
NEUTRAL_PROCESSING: JsonObject = {
    'brightness_percent': 100,
    'style': {'kind': 'custom'},
    'custom_style': {
        'smoothing_seconds': 0,
        'cut_response_percent': 100,
        'brightness_ramp_seconds': 0,
        'spatial_reach_percent': 10,
        'color_focus_percent': 0,
        'saturation_boost_percent': 0,
        'dark_floor_percent': 0,
    },
    'remove_black_bars': False,
}


def binary(config: Config) -> str:
    return config.desktop.headless or str(commands.ROOT / 'target/release/prysm-headless')


def build(ctx: Bench) -> str:
    """The setup step that builds a release prysm-headless (or checks the configured binary)."""
    path = Path(binary(ctx.config))
    if ctx.config.desktop.headless or ctx.options.no_build:
        if not path.exists():
            raise Failed(f'{path} does not exist')
        return f'using {path}'
    if not (commands.ROOT / 'frontend/dist/headless').is_dir():
        raise Failed(
            'frontend/dist/headless is missing: run npm --prefix frontend ci && npm --prefix frontend run bundle first'
        )
    ctx.run(commands.cargo_build_headless())
    return f'built {path}'


def settings(output: JsonObject, capture: JsonObject | None = None) -> Settings:
    document: Settings = {'output': output, 'processing': NEUTRAL_PROCESSING}
    if capture:
        document['capture'] = capture
    return document


def wled_output(config: Config, dither: bool = True) -> JsonObject:
    leds = dict[str, Json](config.leds.as_settings())
    return {'kind': 'wled', 'address': config.wled, 'leds': leds, 'dither': dither}


def local_address(towards: str) -> str:
    """This computer's address on the route to `towards` (a UDP connect sends nothing)."""
    host = towards.split(':', maxsplit=1)[0]
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        probe.connect((socket.gethostbyname(host), 9))
        return probe.getsockname()[0]


class Headless:
    """One running prysm-headless; `stop` sends SIGTERM and returns the exit code."""

    def __init__(
        self, ctx: Bench, settings_path: str, listen: str, allow_null_origin: bool = False, name: str = 'headless'
    ) -> None:
        self.ctx = ctx
        self.command = commands.headless(binary(ctx.config), settings_path, listen, allow_null_origin=allow_null_origin)
        port = listen.rsplit(':', 1)[1]
        host = listen.rsplit(':', 1)[0]
        self.url = f'http://{"127.0.0.1" if host == "0.0.0.0" else host}:{port}'
        self.log_path = ctx.out_dir / f'{name}.log'
        self.process: Process | None = None
        self.exit_code: int | str | None = None

    def start(self, seconds: float = 15) -> State:
        with self.log_path.open('ab') as log:
            self.process = self.ctx.transport.spawn(self.command, stdout=log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f'prysm-headless exited {self.process.returncode}; see {self.log_path}')
            try:
                return cast('State', web.get_json(self.url + '/api/v1/state', timeout=1))
            except (web.Unreachable, RuntimeError):
                time.sleep(0.2)
        raise RuntimeError(f'prysm-headless did not answer within {seconds} s; see {self.log_path}')

    def stop(self, seconds: int = 30) -> int | str | None:
        if self.process is None or self.exit_code is not None:
            return self.exit_code
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
        try:
            self.exit_code = self.process.wait(seconds)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            self.exit_code = f'killed after no exit within {seconds} s'
        return self.exit_code


def write_settings(ctx: Bench, name: str, document: Settings) -> Path:
    path = ctx.out_dir / name
    path.write_text(json.dumps(document, indent=2) + '\n')
    return path
