"""The Debian package cell's seam: the package installed on a Linux target over SSH and run as its systemd unit.

`SshDeb` is a `host.Host`. Nothing of Prysm may be there before the run
(`deb.leftovers`: the package, its files and state, a unit, the unit's
`prysm-headless` user or group): the preflight refuses, saying what it
found, or with `[linux] installed = "replace"` the cell backs it up through
its portal and purges it (`DebPrysm`, `installed.py`). Nothing is left after
the run: `close` always purges
the package and fails when `deb.leftovers` still finds anything, which it
reports and leaves for a person. In between the runner copies the package
into its run directory (as the binary cell does), installs it with apt-get
and adds a runtime drop-in, the package's documented override, so the unit's
own command also gets `--listen LISTEN --metrics`. It restarts the unit and
gives Prysm its settings through the portal, as a person does, so Prysm
writes them into the state directory of the unit's dynamic user itself. The
unit is started and stopped with systemctl; its exit status comes from
systemd and its log from the journal. Every remote command is a `deb.*` (or
shared `linux.*`) template in `commands.py`; sudo is used only by `deb.*`.
Only SSH targets: `[linux] target = "local"` is not implemented for this
cell.

Removing the package cell is removing this module, its templates, its cell
in `options.CELLS` and its checks' section in `host_cells.py`.
"""

import email
import io
from pathlib import Path
import tarfile

from prysm_acceptance import commands, installed
from prysm_acceptance.commands import UnitAction
from prysm_acceptance.config import Installed
from prysm_acceptance.host import (
    DEBIAN_MACHINES,
    SAMPLE_SINCE,
    Plan,
    cleanup,
    device_node,
    listen_url,
    port_in_use,
    route_source,
    run_name,
    save_kernel_usb,
    shown,
    tail,
    target_machine,
    wait_for_state,
    write_settings,
)
from prysm_acceptance.installed import Backup
from prysm_acceptance.options import Source
from prysm_acceptance.prysm_api import Prysm, Settings, State
from prysm_acceptance.transport import Result, Transport

# Where the installed unit's Prysm listens: its default address, as the package runs it without a drop-in.
UNIT_LISTEN = '0.0.0.0:8787'


def deb_control(path: Path) -> dict[str, str]:
    """The control fields of the Debian package at `path` (an `ar` archive with control.tar.xz or .gz); empty
    when they cannot be read."""
    try:
        with path.open('rb') as file:
            if file.read(8) != b'!<arch>\n':
                return {}
            while len(header := file.read(60)) == 60:  # each member: a header, its data, a pad to an even size
                name, size = header[:16].decode().strip().rstrip('/'), int(header[48:58])
                data = file.read(size)
                file.read(size % 2)
                if name.startswith('control.tar'):
                    return _control_fields(data)
    except (OSError, ValueError, tarfile.TarError):
        return {}
    return {}


def _control_fields(control_tar: bytes) -> dict[str, str]:
    """The fields of the `control` file in a package's control.tar (deb822, which the email parser reads)."""
    with tarfile.open(fileobj=io.BytesIO(control_tar)) as tar:
        member = next((m for m in tar.getmembers() if m.name in ('control', './control')), None)
        extracted = tar.extractfile(member) if member else None
        text = extracted.read().decode() if extracted else ''
    return dict(email.message_from_string(text).items())


def unit_properties(text: str) -> dict[str, str]:
    """`systemctl show`'s KEY=VALUE lines as a dict."""
    return dict(line.split('=', 1) for line in text.splitlines() if '=' in line)


def leftovers(text: str) -> list[str]:
    """What `deb.leftovers` found: paths that exist, and dpkg's status unless it says the package is gone."""
    found = [line.strip() for line in text.splitlines() if line.strip()]
    return [line for line in found if line != 'not-installed']


class DebPrysm:
    """The package as it may already be installed on `target` (USER@HOST), for `installed.Existing`: found with
    `deb.leftovers`, backed up through its portal on `UNIT_LISTEN`, purged."""

    name = 'linux'

    def __init__(self, transport: Transport, target: str) -> None:
        self.transport = transport
        self.target = commands.ssh_target(target)
        self.url = listen_url(self.target, UNIT_LISTEN)

    def found(self) -> list[str]:
        return leftovers(self.transport.run(commands.deb_leftovers(self.target)).stdout)

    def backup(self, found: list[str]) -> Backup:
        """The settings and target of the Prysm the unit runs. Refused while an administrator's unit or drop-in
        under /etc is there: the purge would leave it."""
        kept = [path for path in found if path.startswith('/etc/')]
        if kept:
            raise RuntimeError(f'{", ".join(kept)} on {self.target} would outlive the purge: remove it there first')
        state = Prysm(self.url).state()
        return {
            'target': self.name,
            'host': self.target,
            'taken': installed.stamp(),
            'found': found,
            'portal': {'url': self.url, 'settings': state['settings'], 'target': state['target']},
            'luna': None,
            'files': {},
        }

    def remove(self) -> list[str]:
        self.transport.run(commands.deb_purge(self.target))
        return self.found()

    def restore(self, backup: Backup) -> list[str]:
        """The settings into the reinstalled package's Prysm; its lighting is not switched on."""
        portal = backup['portal']
        if portal is None:
            raise RuntimeError('the backup holds no settings')
        reply = Prysm(self.url).replace_settings(portal['settings'])
        if reply.status != 200:
            raise RuntimeError(f'PUT {self.url}/api/v1/settings: HTTP {reply.status} {reply.body[:300]!r}')
        return [f'settings into {self.url}', f'lighting was {portal["target"]}: not switched on; use the portal']


class SshDeb:
    """The package from `deb` installed on `target` (USER@HOST), its unit listening on `listen`. A Prysm found
    installed is refused, or with `installed = "replace"` left for the cell to replace (`existing`, `found`)."""

    def __init__(
        self,
        transport: Transport,
        target: str,
        deb: Path,
        listen: str,
        out_dir: Path,
        wled: str,
        policy: Installed = Installed.REFUSE,
    ) -> None:
        self.transport = transport
        self.target = commands.ssh_target(target)
        self.existing = DebPrysm(transport, target)
        self.policy = policy
        self.found: list[str] = []  # what of Prysm the preflight found, for `replace`
        self.deb = deb
        self.listen = listen
        self.wled = wled
        self.port = int(listen.rsplit(':', 1)[1])
        self.run = run_name(out_dir)
        self.url = listen_url(self.target, listen)
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

    def plan(self, source: Source | None, device: str) -> Plan:
        target = self.target
        return Plan(
            preflight=[
                f'{self.deb.name}: Architecture from its control file, compared with uname -m on the target',
                *shown(commands.linux_probe(target), commands.deb_sudo(target), commands.deb_leftovers(target)),
                '(anything of Prysm on the target, a prysm-headless user or group included: '
                + (
                    'refused, nothing is changed; [linux] installed = "refuse")'
                    if self.policy == Installed.REFUSE
                    else 'replaced after the WLED snapshot; [linux] installed = "replace")'
                ),
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

    def preflight(self, source: Source | None, device: str) -> str | None:
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
        """Why the bench is not ready: Prysm already there (with `refuse`), the device missing, the port taken;
        None when it is."""
        self.found = self.existing.found()
        if self.policy == Installed.REFUSE:
            reason = installed.refusal(self.existing, self.found)
            if reason:
                return reason
        self.device_node, reason = device_node(self.transport, self.target, device)
        if self.device_node is None:
            return reason
        return port_in_use(self.transport, self.target, self.port)

    def deliver(self) -> str:
        self.delivered = True  # from here on `close` purges and removes the run directory, also after a failure
        self.since = int(self._run(commands.deb_deliver(self.target, self.run, str(self.deb))).stdout.split()[-1])
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
        """The drop-in, a restart (the install started the unit without it), then the settings through the
        portal, as a person gives them: Prysm writes them into its state directory itself."""
        write_settings(self.settings_path, document)  # evidence only
        self._run(commands.deb_dropin(self.target, self.listen))
        self.started = True
        self.restart(seconds)
        reply = Prysm(self.url).replace_settings(document)
        if reply.status != 200:
            raise RuntimeError(f'PUT /api/v1/settings: HTTP {reply.status} {reply.body[:300]!r}')
        return Prysm(self.url).state()

    def restart(self, seconds: float = 30) -> State:
        """`systemctl restart`; the state once the unit is active and the API answers."""
        self._run(commands.deb_systemctl(self.target, UnitAction.RESTART))
        unit: dict[str, str] = {}

        def active() -> bool:
            nonlocal unit
            unit = self._unit()
            if unit.get('ActiveState') == 'failed':
                self.fetch()
                raise RuntimeError(f'{commands.DEB_UNIT} failed on {self.target} ({unit}); see {self.log_path}')
            return unit.get('ActiveState') == 'active'

        state = wait_for_state(self.url, seconds, 0.5, active)
        if state is None:
            self.fetch()
            raise RuntimeError(f'{commands.DEB_UNIT} did not answer at {self.url} within {seconds} s ({unit})')
        return state

    def main_user(self) -> str:
        """The user the unit's main process runs as."""
        return self._run(commands.deb_main_user(self.target)).stdout.strip()

    def _unit(self) -> dict[str, str]:
        return unit_properties(self._run(commands.deb_unit(self.target)).stdout)

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
        """Fetch the journal, purge the package, check that nothing of it is left and remove the run directory,
        each also after a failure."""
        if not self.delivered:
            return
        left: list[str] = []

        def purge() -> None:
            nonlocal left
            left = self.existing.remove()

        cleanup(self.fetch, purge, self._remove)
        if left:
            raise RuntimeError(f'the purge left on {self.target}: {", ".join(left)}')

    def _remove(self) -> None:
        self._run(commands.linux_remove(self.target, self.run))
        self.delivered = False
