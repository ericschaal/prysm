"""The host seam: one way to deliver and run Prysm, the same for every host cell's checks.

`Host` is the interface (docs/developers/acceptance-plan.md, One host seam):
whether the target can run the cell, deliver, start with a settings document,
stop, the log, the base URL, and the address WLED should report as the
sender. `LocalBinary` runs the binary on this computer as a child process,
stopped with SIGTERM, its output in a file of the run directory.
`SshBinary` copies it to a run directory on a Linux target and runs it there
over SSH; every remote command is a `linux.*` template in `commands.py`. The
Debian package (`host_deb.SshDeb`) and the image (`host_image.SshImage`) are
the same interface, each in its own module with its own templates.
"""

from collections.abc import Callable
import contextlib
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import platform
import re
import signal
import socket
import subprocess
import time
from typing import Protocol, cast

from prysm_acceptance import commands, web
from prysm_acceptance.commands import Command, RunFile, Signal
from prysm_acceptance.config import Config
from prysm_acceptance.options import Source
from prysm_acceptance.prysm_api import SETTINGS_FORMAT, Settings, State
from prysm_acceptance.transport import Process, Result, Transport
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
# The ELF header's e_machine, named as `uname -m` names the machine.
ELF_MACHINES = {0x3E: 'x86_64', 0xB7: 'aarch64', 0x28: 'armv7l'}
# Debian's and Docker's architecture names, as `uname -m` names the machine.
DEBIAN_MACHINES = {'arm64': 'aarch64', 'amd64': 'x86_64'}
# The oldest FFmpeg (major version) Prysm's file source works with.
FFMPEG_MAJOR = 9
# A kernel log line about the capture card's USB link.
USB_LINE = re.compile(r'usb|uvc', re.IGNORECASE)


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


def elf_machine(binary: Path) -> str | None:
    """The machine an ELF executable is built for, as `uname -m` names it; None when it is not ELF."""
    with binary.open('rb') as file:
        header = file.read(20)
    if len(header) < 20 or header[:4] != b'\x7fELF':
        return None
    machine = int.from_bytes(header[18:20], 'little' if header[5] == 1 else 'big')
    return ELF_MACHINES.get(machine, f'ELF machine 0x{machine:x}')


def ffmpeg_too_old(version: str) -> str | None:
    """Why FFmpeg (`ffmpeg -version`'s output, or `absent`) cannot serve the file source; None when it can."""
    line = version.strip().partition('\n')[0]
    match = re.match(r'ffmpeg version \D*(\d+)\.', line)
    if match is None:
        return f'the file source needs FFmpeg {FFMPEG_MAJOR}+ on the target; found {line or "nothing"!r}'
    if int(match[1]) < FFMPEG_MAJOR:
        return f'the file source needs FFmpeg {FFMPEG_MAJOR}+ on the target; it has FFmpeg {match[1]}'
    return None


def route_source(transport: Transport, target: str, wled: str) -> str:
    """The `src` of the target's route to WLED: the sender WLED sees from a process on the target (also from a
    container behind Docker's NAT, which masquerades as that address)."""
    host = wled.split(':', maxsplit=1)[0]
    route = transport.run(commands.linux_route(target, socket.gethostbyname(host))).stdout
    match = re.search(r'\bsrc (\S+)', route)
    if match is None:
        raise RuntimeError(f'no source address in the route to WLED on {target}: {route.strip()!r}')
    return match[1]


def target_machine(transport: Transport, target: str) -> tuple[str | None, str]:
    """`uname -m` on the target, or None with why ssh failed or the target is not Linux."""
    probe = transport.run(commands.linux_probe(target), check=False)
    if not probe.ok:
        return None, f'ssh {target} failed (BatchMode, no prompts): {(probe.stderr or probe.stdout).strip()[-300:]}'
    kernel, _, machine = probe.stdout.strip().partition(' ')
    if kernel != 'Linux':
        return None, f'{target} runs {kernel or "an unknown kernel"}, not Linux'
    return machine, ''


def device_node(transport: Transport, target: str, device: str) -> tuple[str | None, str]:
    """The node `device` resolves to on the target, or None with why it cannot be captured from."""
    node = transport.run(commands.linux_device(target, device)).stdout.strip()
    if node in ('', 'unreadable'):
        return None, f'capture device {device} is missing on {target} or not readable by its ssh user'
    return node, ''


def port_in_use(transport: Transport, target: str, port: int) -> str | None:
    """Why Prysm cannot listen on `port` on the target; None when the port is free."""
    if transport.run(commands.linux_port_free(target, port)).stdout.strip():
        return f'port {port} is already in use on {target}'
    return None


def save_kernel_usb(transport: Transport, target: str, since: int, path: Path) -> None:
    """The kernel's USB lines on the target since `since` into `path` (a note when journalctl -k is refused)."""
    found = transport.run(commands.linux_kernel(target, since), check=False)
    if found.ok:
        lines = [line for line in found.stdout.splitlines(keepends=True) if USB_LINE.search(line)]
        text = ''.join(lines) or '(no USB lines in the kernel log since the delivery)\n'
    else:
        text = f'(journalctl -k failed, exit {found.returncode}: {found.stderr.strip()[-300:]})\n'
    path.write_text(text)


def run_name(out_dir: Path) -> str:
    """The run directory's name on a target: the local run's and the cell's (`out_dir`)."""
    return f'{out_dir.parent.name}-{out_dir.name}'


def listen_url(host: str, listen: str) -> str:
    """Prysm's base URL from here when it listens on `listen` on `host` (or USER@HOST): `host` for 0.0.0.0."""
    address, port = listen.rsplit(':', 1)
    return f'http://{host.rsplit("@", 1)[-1] if address == "0.0.0.0" else address}:{port}'


def write_settings(path: Path, document: Settings) -> None:
    """`document` as a settings file: its format number, then its fields."""
    path.write_text(json.dumps({'format': SETTINGS_FORMAT, **document}, indent=2) + '\n')


def shown(*found: Command) -> list[str]:
    """Commands as the dry run prints them."""
    return [f'$ {command.display()}' for command in found]


def wait_for_state(url: str, seconds: float, interval: float, ready: Callable[[], bool]) -> State | None:
    """Prysm's state once `ready()` (its process is up; it raises when that process is gone) and the API
    answers; None when `seconds` pass first."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if ready():
            try:
                return cast('State', web.get_json(url + '/api/v1/state', timeout=1))
            except (web.Unreachable, RuntimeError):
                pass
        time.sleep(interval)
    return None


def cleanup(*steps: Callable[[], object]) -> None:
    """Each step in order, every one also after an earlier one failed; a failure is raised after the last step."""
    with contextlib.ExitStack() as stack:
        for step in reversed(steps):
            stack.callback(step)


@dataclass(frozen=True)
class Plan:
    """What a seam does, for the dry run: the preflight, the start, and the stop with its cleanup."""

    preflight: list[str] = field(default_factory=list[str])
    start: list[str] = field(default_factory=list[str])
    stop: list[str] = field(default_factory=list[str])


class Sender(Protocol):
    """What drives WLED in a cell, as the shared checks see it: a host's Prysm, or a TV's."""

    def sender(self) -> str:
        """The address WLED reports as the sender of Prysm's DDP."""
        ...

    def fetch(self) -> None:
        """Bring what the target keeps (Prysm's log, the kernel's USB lines) into the run directory."""
        ...


class Host(Sender, Protocol):
    """Prysm delivered to and running on a platform, as the checks see it.

    `source` None: no capture of its own to check (a slim TV cell's receiver, which captures the TV's
    stream)."""

    @property
    def url(self) -> str:
        """Prysm's base URL, as this computer reaches it."""
        ...

    @property
    def log_path(self) -> Path:
        """Where Prysm's log is kept in the run directory."""
        ...

    def plan(self, source: Source | None, device: str) -> Plan:
        """What `preflight`, `deliver`, `start`, `stop` and `close` do, for the dry run."""
        ...

    def preflight(self, source: Source | None, device: str) -> str | None:
        """Why the target cannot run this cell (reachable, architecture, FFmpeg, device, port); None if it can."""
        ...

    def listed(self, device: str) -> str:
        """`device` as Prysm lists it among its capture candidates (a link resolved to its node)."""
        ...

    def capture_path(self, device: str) -> str:
        """Where Prysm opens the configured `device`: the path itself, or where a container sees it."""
        ...

    def deliver(self) -> str:
        """Put the artifact where it runs; what was done."""
        ...

    def place(self, clip: Path) -> str:
        """Put the clip where Prysm reads it; its path there."""
        ...

    def start(self, document: Settings, seconds: float = 15) -> State:
        """Write the settings, start Prysm and wait until its API answers."""
        ...

    def stop(self, seconds: int = 30) -> int | str | None:
        """Stop Prysm (SIGTERM, KILL after `seconds`); its exit code, or why there is none. Safe to repeat."""
        ...

    def logs(self, lines: int = 20) -> str:
        """The end of Prysm's log."""
        ...

    def close(self) -> None:
        """Fetch, then remove what `deliver` put on the target. Safe to repeat."""
        ...


def tail(path: Path, lines: int) -> str:
    if not path.exists():
        return ''
    return ''.join(path.read_text(errors='replace').splitlines(keepends=True)[-lines:])


class LocalBinary:
    """The binary on this computer, a child process listening on `listen`."""

    def __init__(
        self,
        transport: Transport,
        binary: Path,
        listen: str,
        out_dir: Path,
        wled: str,
    ) -> None:
        self.transport = transport
        self.binary = binary
        self.wled = wled
        self.settings_path = out_dir / 'headless-settings.json'
        self.command = commands.headless(str(binary), str(self.settings_path), listen)
        self.url = listen_url('127.0.0.1', listen)
        self.log_path = out_dir / 'headless.log'
        self.process: Process | None = None
        self.exit_code: int | str | None = None

    def sender(self) -> str:
        return local_address(self.wled)

    def plan(self, source: Source | None, device: str) -> Plan:
        return Plan(
            preflight=[
                'an ELF (Linux) binary only: this computer runs Linux on its machine'
                + (', and the device is readable' if source == Source.DEVICE else '')
            ],
            start=[f'$ {self.command.display()} &'],
            stop=['Prysm stopped with SIGTERM (KILL after 30 s)'],
        )

    def preflight(self, source: Source | None, device: str) -> str | None:
        machine = elf_machine(self.binary)
        if machine is None:
            return None  # not ELF: a Mac binary, which the Mac runs or refuses itself
        if platform.system() != 'Linux':
            return f'{self.binary.name} is a Linux binary; this computer runs {platform.system()}'
        if machine != platform.machine():
            return f'{self.binary.name} is built for {machine}; this computer is {platform.machine()}'
        if source == Source.DEVICE and not os.access(device, os.R_OK):
            return f'capture device {device} is missing or not readable'
        return None

    def listed(self, device: str) -> str:
        return os.path.realpath(device) if device.startswith('/dev/') else device

    def capture_path(self, device: str) -> str:
        return device

    def deliver(self) -> str:
        return 'nothing to deliver: the binary runs where it is'

    def place(self, clip: Path) -> str:
        return str(clip)

    def start(self, document: Settings, seconds: float = 15) -> State:
        write_settings(self.settings_path, document)
        with self.log_path.open('ab') as log:
            process = self.process = self.transport.spawn(self.command, stdout=log, stderr=subprocess.STDOUT)

        def running() -> bool:
            if process.poll() is not None:
                raise RuntimeError(f'prysm-headless exited {process.returncode}; see {self.log_path}')
            return True

        state = wait_for_state(self.url, seconds, 0.2, running)
        if state is None:
            raise RuntimeError(f'prysm-headless did not answer within {seconds} s; see {self.log_path}')
        return state

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
        return tail(self.log_path, lines)

    def fetch(self) -> None:
        """The log is written here already."""

    def close(self) -> None:
        """Nothing was delivered."""


# The PID the dry run shows in place of the one `linux.status` reports.
SAMPLE_PID = 4242
# The target's clock at delivery, as the dry run shows it.
SAMPLE_SINCE = 1


def _code(value: str) -> int | str:
    return int(value) if value.isdigit() else value


class SshBinary:
    """The Linux binary copied to a run directory on `target` (USER@HOST) and run there over SSH.

    The run directory is `commands.run_dir(run)`, named after the local run and
    cell; nothing outside it is written. Prysm listens on `listen` (the LAN), so
    `url` is the target's host with the listen port."""

    def __init__(self, transport: Transport, target: str, binary: Path, listen: str, out_dir: Path, wled: str) -> None:
        self.transport = transport
        self.target = commands.ssh_target(target)
        self.binary = binary
        self.wled = wled
        self.listen = listen
        self.port = int(listen.rsplit(':', 1)[1])
        self.run = run_name(out_dir)
        self.settings_path = out_dir / 'headless-settings.json'
        self.log_path = out_dir / 'headless.log'
        self.kernel_path = out_dir / 'kernel-usb.log'
        self.url = listen_url(self.target, listen)
        self.directory = commands.run_dir(self.run)  # as the remote shell spells it, until `deliver` resolves it
        self.delivered = False
        self.since: int | None = None  # the target's clock when the run directory was made
        self.pid: int | None = None
        self.exit_code: int | str | None = None
        self.device_node: str | None = None

    def _run(self, command: Command, check: bool = True) -> Result:
        return self.transport.run(command, check=check)

    def sender(self) -> str:
        return route_source(self.transport, self.target, self.wled)

    def plan(self, source: Source | None, device: str) -> Plan:
        checks = [
            commands.linux_probe(self.target),
            *([commands.linux_ffmpeg_version(self.target)] if source == Source.FILE else []),
            *([commands.linux_device(self.target, device or '/dev/DEVICE')] if source == Source.DEVICE else []),
            commands.linux_port_free(self.target, self.port),
        ]
        start = [
            commands.linux_deliver(self.target, self.run, str(self.binary)),
            *(
                [commands.linux_put(self.target, self.run, RunFile.CLIP, 'RUN_DIR/CELL/edges.mp4')]
                if source == Source.FILE
                else []
            ),
            commands.linux_put(self.target, self.run, RunFile.SETTINGS, str(self.settings_path)),
            commands.linux_start(self.target, self.run, self.listen),
            commands.linux_status(self.target, self.run),
        ]
        stop = [
            commands.linux_status(self.target, self.run),
            commands.linux_signal(self.target, self.run, SAMPLE_PID, Signal.TERM),
            commands.linux_wait_exit(self.target, self.run),
            commands.linux_signal(self.target, self.run, SAMPLE_PID, Signal.KILL),
            commands.linux_read_log(self.target, self.run),
            commands.linux_kernel(self.target, SAMPLE_SINCE),
            commands.linux_remove(self.target, self.run),
        ]
        return Plan(
            preflight=[f'{self.binary.name}: its ELF machine, compared with uname -m on the target', *shown(*checks)],
            start=[
                *shown(*start),
                f'GET {self.url}/api/v1/state answers while linux.status says running (else fails within 15 s)',
            ],
            stop=[
                *shown(*stop),
                f'({SAMPLE_PID}: the PID linux.status reported; KILL only without an exit within 30 s; @1: the '
                "target's clock at delivery; the kernel's USB lines go to kernel-usb.log, the log to headless.log)",
            ],
        )

    def _system(self) -> str | None:
        """Why the target is unreachable or does not run this binary; None when it does."""
        machine, reason = target_machine(self.transport, self.target)
        if machine is None:
            return reason
        built = elf_machine(self.binary)
        if built != machine:
            return f'{self.binary.name} is built for {built or "no ELF machine"}; {self.target} is {machine}'
        return None

    def preflight(self, source: Source | None, device: str) -> str | None:
        reason = self._system()
        if reason:
            return reason
        if source == Source.FILE:
            reason = ffmpeg_too_old(self._run(commands.linux_ffmpeg_version(self.target)).stdout)
            if reason:
                return reason
        elif source == Source.DEVICE:
            self.device_node, reason = device_node(self.transport, self.target, device)
            if self.device_node is None:
                return reason
        return port_in_use(self.transport, self.target, self.port)

    def listed(self, device: str) -> str:
        return self.device_node or device

    def capture_path(self, device: str) -> str:
        return device

    def deliver(self) -> str:
        self.delivered = True  # from here on `close` removes the run directory, also after a failed copy
        words = self._run(commands.linux_deliver(self.target, self.run, str(self.binary))).stdout.split()
        self.directory, self.since = words[-2], int(words[-1])
        return f'copied {self.binary.name} to {self.target}:{self.directory}'

    def place(self, clip: Path) -> str:
        self._run(commands.linux_put(self.target, self.run, RunFile.CLIP, str(clip)))
        return f'{self.directory}/{RunFile.CLIP}'

    def _status(self) -> tuple[str, str]:
        """`exited CODE`, `running PID` or `starting`, as two words."""
        word, _, value = self._run(commands.linux_status(self.target, self.run)).stdout.strip().partition(' ')
        return word, value

    def start(self, document: Settings, seconds: float = 15) -> State:
        write_settings(self.settings_path, document)
        self._run(commands.linux_put(self.target, self.run, RunFile.SETTINGS, str(self.settings_path)))
        self._run(commands.linux_start(self.target, self.run, self.listen))
        state = wait_for_state(self.url, seconds, 0.3, self._running)
        if state is None:
            self.fetch()
            raise RuntimeError(f'prysm-headless did not answer at {self.url} within {seconds} s; see {self.log_path}')
        return state

    def _running(self) -> bool:
        """Whether this run's own process runs (a leftover Prysm on the port would answer too); raises when it
        exited."""
        word, value = self._status()
        if word == 'exited':
            self.exit_code = _code(value)
            self.fetch()
            raise RuntimeError(f'prysm-headless exited {value} on {self.target}; see {self.log_path}')
        if word == 'running':
            self.pid = int(value)
            return True
        return False

    def stop(self, seconds: int = 30) -> int | str | None:
        if self.exit_code is not None or not self.delivered:
            return self.exit_code
        if self.pid is None:
            word, value = self._status()
            if word == 'exited':
                self.exit_code = _code(value)
                return self.exit_code
            if word != 'running':
                return None  # never started
            self.pid = int(value)
        self._run(commands.linux_signal(self.target, self.run, self.pid, Signal.TERM))
        code = self._run(commands.linux_wait_exit(self.target, self.run, seconds)).stdout.strip()
        if code == 'running':
            self._run(commands.linux_signal(self.target, self.run, self.pid, Signal.KILL))
            self._run(commands.linux_wait_exit(self.target, self.run, 5))
            self.exit_code = f'killed after no exit within {seconds} s'
        else:
            self.exit_code = _code(code)
        return self.exit_code

    def logs(self, lines: int = 20) -> str:
        if self.delivered:
            self._fetch_log()
        return tail(self.log_path, lines)

    def _fetch_log(self) -> None:
        self.log_path.write_text(self._run(commands.linux_read_log(self.target, self.run)).stdout)

    def fetch(self) -> None:
        if not self.delivered:
            return
        self._fetch_log()
        if self.since is not None:
            save_kernel_usb(self.transport, self.target, self.since, self.kernel_path)

    def close(self) -> None:
        if self.delivered:
            cleanup(self.fetch, self._remove)

    def _remove(self) -> None:
        self._run(commands.linux_remove(self.target, self.run))
        self.delivered = False
