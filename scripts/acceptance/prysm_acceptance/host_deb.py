"""The Debian package cell's seam: the package installed on a Linux target over SSH and run as its systemd unit.

`SshDeb` is a `host.Host`. The target is a test bench: nothing of Prysm may
be there before the run (`deb.leftovers`: the package, its files and state,
a unit, a `prysm` user or group; the preflight refuses, saying what it
found), and nothing is left after it: `close` always purges the package and
fails when `deb.leftovers` still finds anything, which it reports and leaves
for a person. In between the runner copies the package into its run
directory (as the binary cell does), installs it with apt-get and adds a
runtime drop-in, the package's documented override, so the unit's own
command also gets `--listen LISTEN --metrics`. It restarts the unit and gives
Prysm its settings through the portal, as a person does, so Prysm writes them
into its state directory itself, whichever user owns it (a static `prysm` or
a DynamicUser). The unit is started and stopped with systemctl; its exit
status comes from systemd and its log from the journal. Every remote command
is a `deb.*` (or shared `linux.*`) template in `commands.py`; sudo is used
only by `deb.*`. Only SSH targets: `[linux] target = "local"` is not
implemented for this cell.

Removing the package cell is removing this module, its templates, its cell
in `options.CELLS` and its checks' section in `host_cells.py`.
"""

import io
import json
from pathlib import Path
import tarfile
import time
from typing import cast

from prysm_acceptance import commands, web
from prysm_acceptance.commands import UnitAction
from prysm_acceptance.host import (
    DEBIAN_MACHINES,
    Plan,
    route_source,
    save_kernel_usb,
    tail,
    target_machine,
)
from prysm_acceptance.options import Source
from prysm_acceptance.prysm_api import Prysm, Settings, State
from prysm_acceptance.transport import Result, Transport

# The start the dry run shows for `@SINCE`.
SAMPLE_SINCE = 1


def deb_control(path: Path) -> dict[str, str]:
    """The control fields of the Debian package at `path` (an `ar` archive with control.tar.xz or .gz); empty
    when they cannot be read."""
    try:
        with path.open('rb') as file:
            if file.read(8) != b'!<arch>\n':
                return {}
            while len(header := file.read(60)) == 60:
                name, size = header[:16].decode().strip().rstrip('/'), int(header[48:58])
                data = file.read(size)
                file.read(size % 2)
                if name.startswith('control.tar'):
                    return _fields(data)
    except (OSError, ValueError, tarfile.TarError):
        return {}
    return {}


def _fields(control_tar: bytes) -> dict[str, str]:
    with tarfile.open(fileobj=io.BytesIO(control_tar)) as tar:
        member = next((m for m in tar.getmembers() if m.name in ('control', './control')), None)
        extracted = tar.extractfile(member) if member else None
        text = extracted.read().decode() if extracted else ''
    found: dict[str, str] = {}
    for line in text.splitlines():
        if line[:1].strip() and ':' in line:
            key, _, value = line.partition(':')
            found[key] = value.strip()
    return found


def unit_properties(text: str) -> dict[str, str]:
    """`systemctl show`'s KEY=VALUE lines as a dict."""
    return dict(line.split('=', 1) for line in text.splitlines() if '=' in line)


def leftovers(text: str) -> list[str]:
    """What `deb.leftovers` found: paths that exist, and dpkg's status unless it says the package is gone."""
    found = [line.strip() for line in text.splitlines() if line.strip()]
    return [line for line in found if line != 'not-installed']


class SshDeb:
    """The package from `deb` installed on `target` (USER@HOST), its unit listening on `listen`."""

    def __init__(self, transport: Transport, target: str, deb: Path, listen: str, out_dir: Path, wled: str) -> None:
        self.transport = transport
        self.target = commands.ssh_target(target)
        self.deb = deb
        self.listen = listen
        self.wled = wled
        self.port = int(listen.rsplit(':', 1)[1])
        self.run = f'{out_dir.parent.name}-{out_dir.name}'
        address = listen.rsplit(':', 1)[0]
        self.url = f'http://{self.target.rsplit("@", 1)[-1] if address == "0.0.0.0" else address}:{self.port}'
        self.settings_path = out_dir / 'headless-settings.json'
        self.log_path = out_dir / 'journal.log'
        self.kernel_path = out_dir / 'kernel-usb.log'
        self.apt_path = out_dir / 'apt.log'
        self.version = ''  # the package's own, from its control file
        self.delivered = False  # the run directory exists: `close` purges and removes it
        self.since: int | None = None
        self.started = False
        self.exit_code: int | str | None = None
        self.device_node: str | None = None

    def _run(self, command: commands.Command, check: bool = True) -> Result:
        return self.transport.run(command, check=check)

    def sender(self) -> str:
        return route_source(self.transport, self.target, self.wled)

    def listed(self, device: str) -> str:
        return self.device_node or device

    def capture_path(self, device: str) -> str:
        return device

    def plan(self, source: Source, device: str) -> Plan:
        def shown(*found: commands.Command) -> list[str]:
            return [f'$ {command.display()}' for command in found]

        target = self.target
        return Plan(
            preflight=[
                f'{self.deb.name}: Architecture from its control file, compared with uname -m on the target',
                *shown(commands.linux_probe(target), commands.deb_sudo(target), commands.deb_leftovers(target)),
                '(refused while anything of Prysm is on the target, a prysm user or group included: the bench '
                'starts clean)',
                *shown(
                    commands.linux_device(target, device or '/dev/DEVICE'), commands.linux_port_free(target, self.port)
                ),
            ],
            start=[
                *shown(
                    commands.deb_deliver(target, self.run, str(self.deb)),
                    commands.deb_install(target, self.run),
                    commands.deb_installed(target),
                ),
                "(dpkg's version must be the control file's; apt-get's output goes to apt.log)",
                *shown(
                    commands.deb_dropin(target, self.listen),
                    commands.deb_systemctl(target, UnitAction.RESTART),
                    commands.deb_unit(target),
                ),
                f'GET {self.url}/api/v1/state answers while the unit is active (else fails within 30 s)',
                f'PUT {self.url}/api/v1/settings with the settings above: Prysm saves them in its state directory',
            ],
            stop=[
                *shown(
                    commands.deb_systemctl(target, UnitAction.STOP),
                    commands.deb_unit(target),
                    commands.deb_journal(target, SAMPLE_SINCE),
                    commands.linux_kernel(target, SAMPLE_SINCE),
                    commands.deb_purge(target),
                    commands.deb_leftovers(target),
                    commands.linux_remove(target, self.run),
                ),
                "(the exit code is the unit's ExecMainStatus; @1: the target's clock at delivery; the journal goes to "
                'journal.log; the purge also runs after a failure; anything deb.leftovers still lists, a prysm user '
                'or group included, fails the cell and is left for a person to remove)',
            ],
        )

    def preflight(self, source: Source, device: str) -> str | None:
        return self._system() or self._clean(device)

    def _system(self) -> str | None:
        """Why the package does not install on the target (architecture, sudo); None when it does."""
        control = deb_control(self.deb)
        arch = control.get('Architecture', '')
        if control.get('Package') != commands.DEB_PACKAGE or arch not in DEBIAN_MACHINES:
            return f'{self.deb.name}: no prysm-headless control file for arm64 or amd64 could be read from it'
        self.version = control.get('Version', '')
        machine, reason = target_machine(self.transport, self.target)
        if machine is None:
            return reason
        if DEBIAN_MACHINES[arch] != machine:
            return f'{self.deb.name} is for {arch}; {self.target} is {machine}'
        if not self._run(commands.deb_sudo(self.target), check=False).ok:
            return f'sudo -n true fails on {self.target}: the package cell needs passwordless sudo'
        return None

    def _clean(self, device: str) -> str | None:
        """Why the bench is not ready: Prysm already there, the device missing, the port taken; None when it is."""
        found = leftovers(self._run(commands.deb_leftovers(self.target)).stdout)
        if found:
            return (
                f'Prysm is already on {self.target}: {", ".join(found)}. The bench must start without it: there, '
                'sudo apt-get purge prysm-headless, and remove what is still listed by hand'
            )
        node = self._run(commands.linux_device(self.target, device)).stdout.strip()
        if node in ('', 'unreadable'):
            return f'capture device {device} is missing on {self.target} or not readable by its ssh user'
        self.device_node = node
        if self._run(commands.linux_port_free(self.target, self.port)).stdout.strip():
            return f'port {self.port} is already in use on {self.target}'
        return None

    def deliver(self) -> str:
        self.delivered = True  # from here on `close` purges and removes the run directory, also after a failure
        words = self._run(commands.deb_deliver(self.target, self.run, str(self.deb))).stdout.split()
        self.since = int(words[-1])
        apt = self._run(commands.deb_install(self.target, self.run), check=False)
        self.apt_path.write_text(apt.stdout + apt.stderr)
        if not apt.ok:
            raise RuntimeError(f'apt-get install exited {apt.returncode} on {self.target}; see {self.apt_path}')
        status = self._run(commands.deb_installed(self.target)).stdout.partition('\n')[0].strip()
        if status != f'installed {self.version}':
            raise RuntimeError(f'dpkg reports {status!r} on {self.target}; the package is {self.version}')
        return f'installed {self.deb.name} ({self.version}) on {self.target} with apt-get'

    def place(self, clip: Path) -> str:
        raise RuntimeError('the package cell has no file source')

    def start(self, document: Settings, seconds: float = 30) -> State:
        """The drop-in, a restart (the install started the unit unconfigured, without it), then the settings
        through the portal, as a person gives them: Prysm writes them to its state directory itself, whoever
        owns it (a static user or a DynamicUser)."""
        self.settings_path.write_text(json.dumps(document, indent=2) + '\n')  # evidence only
        self._run(commands.deb_dropin(self.target, self.listen))
        self.started = True
        self._run(commands.deb_systemctl(self.target, UnitAction.RESTART))
        self._answering(seconds)
        reply = Prysm(self.url).replace_settings(document)
        if reply.status != 200:
            raise RuntimeError(f'PUT /api/v1/settings: HTTP {reply.status} {reply.body[:300]!r}')
        return Prysm(self.url).state()

    def restart(self, seconds: float = 30) -> State:
        """`systemctl restart`; the state once the API answers again."""
        self._run(commands.deb_systemctl(self.target, UnitAction.RESTART))
        return self._answering(seconds)

    def main_user(self) -> str:
        """The user the unit's main process runs as."""
        return self._run(commands.deb_main_user(self.target)).stdout.strip()

    def _unit(self) -> dict[str, str]:
        return unit_properties(self._run(commands.deb_unit(self.target)).stdout)

    def _answering(self, seconds: float) -> State:
        deadline = time.monotonic() + seconds
        unit: dict[str, str] = {}
        while time.monotonic() < deadline:
            unit = self._unit()
            if unit.get('ActiveState') == 'failed':
                self.fetch()
                raise RuntimeError(f'{commands.DEB_UNIT} failed on {self.target} ({unit}); see {self.log_path}')
            if unit.get('ActiveState') == 'active':
                try:
                    return cast('State', web.get_json(self.url + '/api/v1/state', timeout=1))
                except (web.Unreachable, RuntimeError):
                    pass
            time.sleep(0.5)
        self.fetch()
        raise RuntimeError(f'{commands.DEB_UNIT} did not answer at {self.url} within {seconds} s ({unit})')

    def stop(self, seconds: int = 30) -> int | str | None:
        """`systemctl stop` (SIGTERM; systemd kills after the unit's TimeoutStopSec, 30 s); `seconds` is the
        unit's, not the runner's."""
        if self.exit_code is not None or not self.started:
            return self.exit_code
        self._run(commands.deb_systemctl(self.target, UnitAction.STOP))
        unit = self._unit()
        if unit.get('Result') == 'timeout':
            self.exit_code = 'killed by systemd after no exit within TimeoutStopSec (30 s)'
        elif unit.get('ExecMainCode') == '1':  # CLD_EXITED: ExecMainStatus is the exit code
            self.exit_code = int(unit.get('ExecMainStatus', '-1'))
        else:
            self.exit_code = f'ended by signal {unit.get("ExecMainStatus")} (Result {unit.get("Result")})'
        return self.exit_code

    def logs(self, lines: int = 20) -> str:
        self.fetch()
        return tail(self.log_path, lines)

    def fetch(self) -> None:
        if not self.delivered or self.since is None:
            return
        self.log_path.write_text(self._run(commands.deb_journal(self.target, self.since)).stdout)
        save_kernel_usb(self.transport, self.target, self.since, self.kernel_path)

    def close(self) -> None:
        """Fetch the journal, purge the package (also after a failure), check nothing of it is left, remove the
        run directory."""
        if not self.delivered:
            return
        left: list[str] = []
        try:
            self.fetch()
        finally:
            try:
                self._run(commands.deb_purge(self.target))
                left = leftovers(self._run(commands.deb_leftovers(self.target)).stdout)
            finally:
                self._run(commands.linux_remove(self.target, self.run))
                self.delivered = False
        if left:
            raise RuntimeError(f'the purge left on {self.target}: {", ".join(left)}')
