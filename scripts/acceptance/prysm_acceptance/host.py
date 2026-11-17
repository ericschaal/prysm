"""The host seam: one way to deliver and run Prysm, the same for every host cell's checks.

`Host` is the interface (docs/developers/acceptance-plan.md, One host seam):
deliver, start with a settings document, stop, the log, the base URL, and the
address WLED should report as the sender. `LocalBinary` is the one delivery
so far: the binary on this computer as a child process, stopped with SIGTERM,
its output in a file of the run directory. A binary over SSH, the Debian
package and the image are later implementations of the same interface.
"""

import json
from pathlib import Path
import signal
import socket
import subprocess
import time
from typing import Protocol, cast

from prysm_acceptance import commands, web
from prysm_acceptance.commands import Command
from prysm_acceptance.config import Config
from prysm_acceptance.prysm_api import Settings, State
from prysm_acceptance.transport import Process, Transport
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


def settings(output: JsonObject, capture: JsonObject | None = None) -> Settings:
    document: Settings = {'output': output, 'processing': NEUTRAL_PROCESSING}
    if capture:
        document['capture'] = capture
    return document


def wled_output(config: Config) -> JsonObject:
    leds = dict[str, Json](config.leds.as_settings())
    return {'kind': 'wled', 'address': config.wled, 'leds': leds, 'dither': True}


def local_address(towards: str) -> str:
    """This computer's address on the route to `towards` (a UDP connect sends nothing)."""
    host = towards.split(':', maxsplit=1)[0]
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        probe.connect((socket.gethostbyname(host), 9))
        return probe.getsockname()[0]


class Host(Protocol):
    """Prysm delivered to and running on a platform, as the checks see it."""

    @property
    def url(self) -> str:
        """Prysm's base URL, as this computer reaches it."""
        ...

    @property
    def log_path(self) -> Path:
        """Where Prysm's log is kept in the run directory."""
        ...

    def sender(self) -> str:
        """The address WLED reports as the sender of Prysm's DDP."""
        ...

    def commands(self) -> list[Command]:
        """The commands `deliver` and `start` run, for the dry run."""
        ...

    def deliver(self) -> str:
        """Put the artifact where it runs; what was done."""
        ...

    def start(self, document: Settings, seconds: float = 15) -> State:
        """Write the settings, start Prysm and wait until its API answers."""
        ...

    def stop(self, seconds: int = 30) -> int | str | None:
        """Stop Prysm (SIGTERM, KILL after `seconds`); its exit code, or why there is none."""
        ...

    def logs(self, lines: int = 20) -> str:
        """The end of Prysm's log."""
        ...


class LocalBinary:
    """The binary on this computer, a child process listening on `listen`."""

    def __init__(
        self,
        transport: Transport,
        binary: Path,
        listen: str,
        out_dir: Path,
        wled: str,
        name: str = 'headless',
        allow_null_origin: bool = False,
    ) -> None:
        self.transport = transport
        self.wled = wled
        self.settings_path = out_dir / f'{name}-settings.json'
        self.command = commands.headless(
            str(binary), str(self.settings_path), listen, allow_null_origin=allow_null_origin
        )
        host, port = listen.rsplit(':', 1)
        self.url = f'http://{"127.0.0.1" if host == "0.0.0.0" else host}:{port}'
        self.log_path = out_dir / f'{name}.log'
        self.process: Process | None = None
        self.exit_code: int | str | None = None

    def sender(self) -> str:
        return local_address(self.wled)

    def commands(self) -> list[Command]:
        return [self.command]

    def deliver(self) -> str:
        return 'nothing to deliver: the binary runs where it is'

    def start(self, document: Settings, seconds: float = 15) -> State:
        self.settings_path.write_text(json.dumps(document, indent=2) + '\n')
        with self.log_path.open('ab') as log:
            self.process = self.transport.spawn(self.command, stdout=log, stderr=subprocess.STDOUT)
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

    def logs(self, lines: int = 20) -> str:
        if not self.log_path.exists():
            return ''
        return ''.join(self.log_path.read_text(errors='replace').splitlines(keepends=True)[-lines:])
