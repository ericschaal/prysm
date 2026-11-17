"""The image cell's seam: the Docker image loaded on a Linux target over SSH and run with its compose file.

`SshImage` is a `host.Host`. The target is a test bench: the image tag under
test (`commands.IMAGE_TAG`) and the run's compose project must not be there
before the run (the preflight refuses), and neither is left after it: `close`
always takes the project down with its containers, network and volume and
removes the loaded image. Docker and Debian's compose plugin run as the ssh
user (in the `docker` group), without sudo.

The container runs `packaging/docker/compose.yaml` as it is, copied into the
run directory, with the runner's values in a second compose file next to it
(`compose.acceptance.yaml`), the way compose files are designed to be
extended: no restart policy, `--metrics` as the command (the compose file's
own `command` slot), the configured device in place of the file's
`/dev/video0` (at the same path in the container, where Prysm finds it) and
the device's group. Those two replace the file's entries with compose's
`!override` tag (compose 2.24.4 or newer: a plain second file would add to
them). The settings are written into the project's volume as the image's
user, as `packaging/docker/smoke.sh` does. Prysm listens on its default port,
which the compose file publishes (`commands.IMAGE_PORT`).

The sender WLED sees is the target's own address: compose.yaml uses a bridge
network, whose traffic to the LAN Docker masquerades as the target's address
on the route to WLED (and with `network_mode: host` it is that address too).

Only SSH targets: `[linux] target = "local"` is not implemented for this cell.
Removing the image cell is removing this module, its templates, its cell in
`options.CELLS` and its checks' section in `host_cells.py`.
"""

import json
from pathlib import Path
import re
import tarfile
from typing import cast

from prysm_acceptance import commands
from prysm_acceptance.commands import ComposeAction, RunFile
from prysm_acceptance.host import (
    DEBIAN_MACHINES,
    SAMPLE_SINCE,
    Plan,
    cleanup,
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
from prysm_acceptance.options import Source
from prysm_acceptance.prysm_api import Settings, State
from prysm_acceptance.transport import Result, Transport

COMPOSE_FILE = commands.ROOT / 'packaging' / 'docker' / 'compose.yaml'
# Where compose.yaml puts the capture device in the container, and so where Prysm (which lists /dev/video*)
# finds it.
CONTAINER_DEVICE = '/dev/video0'
# The oldest compose that knows `!override`.
COMPOSE_OVERRIDE = (2, 24, 4)


def image_archive(path: Path) -> tuple[str, list[str]]:
    """The architecture and the tags of the image in a `docker load` archive; empty when they cannot be read."""
    try:
        with tarfile.open(path) as tar:
            manifest = cast('list[dict[str, object]]', json.load(_member(tar, 'manifest.json')))
            config = cast('dict[str, object]', json.load(_member(tar, str(manifest[0]['Config']))))
    except (OSError, ValueError, KeyError, IndexError, tarfile.TarError):
        return '', []
    tags = manifest[0].get('RepoTags')
    # Docker's short names: an archive may spell the tag in full.
    names = [str(tag).removeprefix('docker.io/library/') for tag in cast('list[object]', tags or [])]
    return str(config.get('architecture', '')), names


def _member(tar: tarfile.TarFile, name: str) -> tarfile.ExFileObject:
    found = tar.extractfile(name)
    if found is None:
        raise KeyError(name)
    return cast('tarfile.ExFileObject', found)


def compose_version(text: str) -> tuple[int, ...]:
    """`docker compose version --short` (`2.26.1`, `v2.26.1`) as numbers; () when it is none."""
    match = re.search(r'(\d+)\.(\d+)\.(\d+)', text)
    return tuple(int(part) for part in match.groups()) if match else ()


def override(device: str, group: str) -> str:
    """The runner's compose file over compose.yaml (JSON strings are YAML strings)."""
    return '\n'.join(
        [
            '# Written by the acceptance runner for one run, over packaging/docker/compose.yaml.',
            'services:',
            '  prysm:',
            '    restart: "no"',
            '    command: ["--metrics"]',
            f'    devices: !override [{json.dumps(f"{commands.v4l2_device(device)}:{CONTAINER_DEVICE}")}]',
            f'    group_add: !override [{json.dumps(group)}]',
            '',
        ]
    )


class SshImage:
    """The image from `image` loaded on `target` (USER@HOST) and run in the run's compose project."""

    def __init__(self, transport: Transport, target: str, image: Path, out_dir: Path, wled: str) -> None:
        self.transport = transport
        self.target = commands.ssh_target(target)
        self.image = image
        self.wled = wled
        self.port = commands.IMAGE_PORT
        self.run = run_name(out_dir)
        self.project = commands.compose_project(self.run)
        self.url = f'http://{self.target.rsplit("@", 1)[-1]}:{self.port}'
        self.settings_path = out_dir / 'headless-settings.json'
        self.override_path = out_dir / RunFile.OVERRIDE
        self.log_path = out_dir / 'container.log'
        self.kernel_path = out_dir / 'kernel-usb.log'
        self.device = ''
        self.group = ''  # the device node's group ID on the target
        self.delivered = False  # the run directory exists
        self.since: int | None = None
        self.image_id: str | None = None  # what the run loaded, for the report
        self.composed = False  # the compose files are in the run directory: `close` takes the project down
        self.exit_code: int | str | None = None

    def _run(self, command: commands.Command, check: bool = True) -> Result:
        return self.transport.run(command, check=check)

    def _compose(self, action: ComposeAction, check: bool = True) -> Result:
        return self._run(commands.image_compose(self.target, self.run, action), check)

    def sender(self) -> str:
        return route_source(self.transport, self.target, self.wled)

    def listed(self, device: str) -> str:
        return CONTAINER_DEVICE

    def capture_path(self, device: str) -> str:
        return CONTAINER_DEVICE

    def plan(self, source: Source | None, device: str) -> Plan:
        target, run = self.target, self.run

        def compose(*actions: ComposeAction) -> list[str]:
            return shown(*(commands.image_compose(target, run, action) for action in actions))

        return Plan(
            preflight=[
                f'{self.image.name}: architecture and tags from its manifest, compared with uname -m on the target '
                f'and {commands.IMAGE_TAG}',
                *shown(commands.linux_probe(target), commands.image_docker(target)),
                f'(docker info and docker compose {".".join(map(str, COMPOSE_OVERRIDE))}+ as the ssh user, no sudo)',
                *shown(commands.image_present(target, run)),
                f'(refused while {commands.IMAGE_TAG} or project {self.project} exists: the bench starts clean)',
                *shown(
                    commands.image_device(target, device or '/dev/DEVICE'), commands.linux_port_free(target, self.port)
                ),
            ],
            start=[
                *shown(commands.image_deliver(target, run, str(self.image)), commands.image_load(target, run)),
                f'write RUN_DIR/CELL/{RunFile.OVERRIDE}: restart "no", command ["--metrics"], devices !override '
                f"[DEVICE:{CONTAINER_DEVICE}], group_add !override [the device's GID]",
                *shown(
                    commands.linux_put(target, run, RunFile.COMPOSE, str(COMPOSE_FILE)),
                    commands.linux_put(target, run, RunFile.OVERRIDE, str(self.override_path)),
                    commands.linux_put(target, run, RunFile.SETTINGS, str(self.settings_path)),
                ),
                *compose(ComposeAction.SEED, ComposeAction.UP, ComposeAction.STATE),
                f'GET {self.url}/api/v1/state answers while the container runs (else fails within 30 s)',
            ],
            stop=[
                *compose(ComposeAction.STOP, ComposeAction.STATE, ComposeAction.LOGS),
                *shown(commands.linux_kernel(target, SAMPLE_SINCE)),
                *compose(ComposeAction.DOWN),
                *shown(commands.image_remove(target), commands.linux_remove(target, run)),
                "(the exit code is the container's, after compose's SIGTERM and 30 s grace; the logs go to "
                'container.log; the project, its volume and the image tag (none was there before the run) are '
                'removed also after a failure)',
            ],
        )

    def preflight(self, source: Source | None, device: str) -> str | None:
        return self._system() or self._clean(device)

    def _system(self) -> str | None:
        """Why the image does not run on the target (architecture, Docker, compose); None when it does."""
        arch, tags = image_archive(self.image)
        if arch not in DEBIAN_MACHINES or commands.IMAGE_TAG not in tags:
            return f'{self.image.name}: no {commands.IMAGE_TAG} image for arm64 or amd64 could be read from it'
        machine, reason = target_machine(self.transport, self.target)
        if machine is None:
            return reason
        if DEBIAN_MACHINES[arch] != machine:
            return f'{self.image.name} is for {arch}; {self.target} is {machine}'
        docker = self._run(commands.image_docker(self.target), check=False)
        if not docker.ok:
            return (
                f'docker info or docker compose version failed on {self.target} (Docker and the compose plugin, '
                f'the ssh user in the docker group): {(docker.stderr or docker.stdout).strip()[-300:]}'
            )
        reported = docker.stdout.strip().rpartition('\n')[2]
        if compose_version(reported) < COMPOSE_OVERRIDE:
            wanted = '.'.join(map(str, COMPOSE_OVERRIDE))
            return f'docker compose {reported!r} on {self.target}: the image cell needs {wanted}+ (!override)'
        return None

    def _clean(self, device: str) -> str | None:
        """Why the bench is not ready: the image or project already there, the device missing, the port taken."""
        present = self._run(commands.image_present(self.target, self.run)).stdout.split()
        if present:
            return (
                f'{commands.IMAGE_TAG} or compose project {self.project} is already on {self.target} '
                f'({", ".join(present)}); the bench must start without it: there, docker image rm {commands.IMAGE_TAG}'
            )
        group = self._run(commands.image_device(self.target, device)).stdout.strip()
        if not group.isdigit():
            return f'capture device {device} is missing on {self.target}'
        self.device, self.group = device, group
        return port_in_use(self.transport, self.target, self.port)

    def deliver(self) -> str:
        self.delivered = True  # from here on `close` removes the run directory, also after a failure
        self.since = int(self._run(commands.image_deliver(self.target, self.run, str(self.image))).stdout.split()[-1])
        self.image_id = self._run(commands.image_load(self.target, self.run)).stdout.split()[-1]
        return f'loaded {self.image.name} on {self.target} as {commands.IMAGE_TAG} ({self.image_id})'

    def place(self, clip: Path) -> str:
        raise RuntimeError('the image cell has no file source')

    def start(self, document: Settings, seconds: float = 30) -> State:
        write_settings(self.settings_path, document)
        self.override_path.write_text(override(self.device, self.group))
        self.composed = True
        for file, local in (
            (RunFile.COMPOSE, COMPOSE_FILE),
            (RunFile.OVERRIDE, self.override_path),
            (RunFile.SETTINGS, self.settings_path),
        ):
            self._run(commands.linux_put(self.target, self.run, file, str(local)))
        self._compose(ComposeAction.SEED)
        self._compose(ComposeAction.UP)
        return self._answering(seconds)

    def recreate(self, seconds: float = 30) -> State:
        """A new container from the same project (`up --force-recreate`); the state once the API answers."""
        self._compose(ComposeAction.RECREATE)
        return self._answering(seconds)

    def _state(self) -> tuple[str, str]:
        """The container's state (`running`, `exited`, ...) and exit code."""
        state, _, code = self._compose(ComposeAction.STATE).stdout.strip().partition(' ')
        return state, code

    def _answering(self, seconds: float) -> State:
        state = ''

        def running() -> bool:
            nonlocal state
            state, code = self._state()
            if state in ('exited', 'dead'):
                self.fetch()
                raise RuntimeError(f'the container {state} with code {code} on {self.target}; see {self.log_path}')
            return state == 'running'

        answer = wait_for_state(self.url, seconds, 0.5, running)
        if answer is None:
            self.fetch()
            raise RuntimeError(f'the container ({state or "none"}) did not answer at {self.url} within {seconds} s')
        return answer

    def stop(self, seconds: int = 30) -> int | str | None:
        """`docker compose stop` (SIGTERM, KILL after compose.yaml's stop_grace_period, 30 s); the container's
        exit code. `seconds` is the compose file's, not the runner's."""
        if self.exit_code is not None or not self.composed:
            return self.exit_code
        self._compose(ComposeAction.STOP)
        state, code = self._state()
        if state != 'exited' or not code.isdigit():
            self.exit_code = f'container {state or "gone"} after the stop'
        elif code == '137':
            self.exit_code = 'killed after no exit within the stop grace period (30 s)'
        else:
            self.exit_code = int(code)
        return self.exit_code

    def logs(self, lines: int = 20) -> str:
        self.fetch()
        return tail(self.log_path, lines)

    def fetch(self) -> None:
        if self.composed:
            self.log_path.write_text(self._compose(ComposeAction.LOGS, check=False).stdout)
        if self.delivered and self.since is not None:
            save_kernel_usb(self.transport, self.target, self.since, self.kernel_path)

    def close(self) -> None:
        """Fetch the logs; take the project down with its volume, remove the loaded image and the run directory,
        each also after a failure."""
        if self.delivered:
            cleanup(self.fetch, self._down, self._remove_image, self._remove)

    def _down(self) -> None:
        if self.composed:
            self._compose(ComposeAction.DOWN)
            self.composed = False

    def _remove_image(self) -> None:
        self._run(commands.image_remove(self.target))  # also after an interrupted load

    def _remove(self) -> None:
        self._run(commands.linux_remove(self.target, self.run))
        self.delivered = False
