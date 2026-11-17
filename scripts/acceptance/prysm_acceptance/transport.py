"""Runs `commands.Command`s: for real, refused (dry run), or from a transcript (tests)."""

from collections.abc import Callable, Iterable
from contextlib import nullcontext
from dataclasses import dataclass
import os
from pathlib import Path
import re
import subprocess
import threading
import time
from typing import IO, Protocol, TypeAlias

from prysm_acceptance.commands import Command

# Where a spawned command's output goes: a file, or `subprocess.STDOUT` for stderr.
Sink: TypeAlias = IO[bytes] | int


@dataclass
class Result:
    returncode: int
    stdout: str
    stderr: str = ''
    seconds: float = 0.0

    @property
    def ok(self) -> bool:
        return self.returncode == 0


class CommandFailed(RuntimeError):
    def __init__(self, command: Command, result: Result) -> None:
        super().__init__(
            f'{command.template} exited {result.returncode}: {(result.stderr or result.stdout).strip()[-400:]}'
        )
        self.result = result


class Process(Protocol):
    """A command started in the background (`spawn`), as `subprocess.Popen` is."""

    @property
    def returncode(self) -> int | None: ...
    def poll(self) -> int | None: ...
    def wait(self, timeout: float | None = None) -> int: ...
    def send_signal(self, sig: int) -> None: ...
    def kill(self) -> None: ...


class Transport(Protocol):
    """What runs the commands of a run."""

    def run(self, command: Command, check: bool = True) -> Result: ...

    def spawn(self, command: Command, stdout: Sink, stderr: Sink) -> Process: ...


class Real:
    """Runs commands with their timeouts. Every command passes through `log`."""

    def __init__(self, log: Callable[[str], None] = print) -> None:
        self.log = log

    def run(self, command: Command, check: bool = True) -> Result:
        if command.background:
            raise ValueError(f'{command.template} runs in the background: use spawn')
        self.log('$ ' + command.display())
        start = time.monotonic()
        result = self._held(command) if command.hold_stdin else self._plain(command)
        result.seconds = time.monotonic() - start
        if check and not result.ok:
            raise CommandFailed(command, result)
        return result

    def spawn(self, command: Command, stdout: Sink, stderr: Sink) -> Process:
        if not command.background:
            raise ValueError(f'{command.template} is not a background command')
        self.log('$ ' + command.display() + ' &')
        return subprocess.Popen(
            command.argv, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, env={**os.environ, **command.env}
        )

    @staticmethod
    def _plain(command: Command) -> Result:
        with Path(command.stdin_file).open('rb') if command.stdin_file else nullcontext(subprocess.DEVNULL) as stdin:
            try:
                done = subprocess.run(
                    command.argv,
                    stdin=stdin,
                    capture_output=True,
                    timeout=command.timeout,
                    env={**os.environ, **command.env},
                    check=False,
                )
            except subprocess.TimeoutExpired as error:
                return Result(124, _text(error.stdout), f'timed out after {command.timeout:.0f} s')
        return Result(done.returncode, _text(done.stdout), _text(done.stderr))

    @staticmethod
    def _held(command: Command) -> Result:
        """Keep stdin open (the TV's Luna client replies only then); stop at `until`, `fail`, exit or timeout."""
        process = subprocess.Popen(
            command.argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env={**os.environ, **command.env},
        )
        out: list[str] = []
        err: list[str] = []
        readers = [
            threading.Thread(target=_drain, args=(process.stdout, out), daemon=True),
            threading.Thread(target=_drain, args=(process.stderr, err), daemon=True),
        ]
        for reader in readers:
            reader.start()
        deadline = time.monotonic() + command.timeout
        code = None
        while time.monotonic() < deadline:
            text = ''.join(out)
            if command.fail and re.search(command.fail, text):
                code = 1
                break
            if command.until and re.search(command.until, text):
                code = 0
                break
            if process.poll() is not None:
                code = process.returncode
                break
            time.sleep(0.2)
        if process.poll() is None:
            process.kill()
        process.wait()
        for reader in readers:
            reader.join(2)
        for pipe in (process.stdin, process.stdout, process.stderr):
            if pipe is not None:
                pipe.close()
        if code is None:
            return Result(124, ''.join(out), ''.join(err) + f'\ntimed out after {command.timeout:.0f} s')
        if command.until and code == 0 and not re.search(command.until, ''.join(out)):
            code = 1
        return Result(code, ''.join(out), ''.join(err))


def _drain(stream: IO[bytes], sink: list[str]) -> None:
    for line in iter(stream.readline, b''):
        sink.append(line.decode(errors='replace'))  # noqa: PERF401 -- line by line: the caller reads `sink` meanwhile


def _text(data: bytes | str | None) -> str:
    return data.decode(errors='replace') if isinstance(data, bytes) else (data or '')


class Refusing:
    """The dry run's transport: executing anything is a bug."""

    def run(self, command: Command, check: bool = True) -> Result:
        raise RuntimeError(f'dry run: refused to execute {command.template}')

    def spawn(self, command: Command, stdout: Sink, stderr: Sink) -> Process:
        raise RuntimeError(f'dry run: refused to start {command.template}')


class Transcript:
    """A recorded exchange for tests: each call must match the next entry's
    template; its result is returned and the command recorded."""

    def __init__(self, entries: Iterable[tuple[str, Result]]) -> None:
        self.entries = list(entries)
        self.calls: list[Command] = []

    def run(self, command: Command, check: bool = True) -> Result:
        self.calls.append(command)
        if not self.entries:
            raise AssertionError(f'unexpected command {command.template}')
        template, result = self.entries.pop(0)
        if template != command.template:
            raise AssertionError(f'expected {template}, got {command.template}')
        if check and not result.ok:
            raise CommandFailed(command, result)
        return result

    def spawn(self, command: Command, stdout: Sink, stderr: Sink) -> Process:
        raise AssertionError(f'unexpected background command {command.template}')
