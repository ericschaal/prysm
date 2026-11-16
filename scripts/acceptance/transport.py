"""Runs `commands.Command`s: for real, refused (dry run), or from a transcript (tests)."""
from dataclasses import dataclass
import os
import re
import subprocess
import threading
import time


@dataclass
class Result:
    returncode: int
    stdout: str
    stderr: str = ''
    seconds: float = 0.0

    @property
    def ok(self):
        return self.returncode == 0


class CommandFailed(RuntimeError):
    def __init__(self, command, result):
        super().__init__(f'{command.template} exited {result.returncode}: '
                         f'{(result.stderr or result.stdout).strip()[-400:]}')
        self.result = result


class Real:
    """Runs commands with their timeouts. Every command passes through `log`."""

    def __init__(self, log=print):
        self.log = log

    def run(self, command, check=True):
        if command.background:
            raise ValueError(f'{command.template} runs in the background: use spawn')
        self.log('$ ' + command.display())
        start = time.monotonic()
        if command.hold_stdin:
            result = self._held(command)
        else:
            result = self._plain(command)
        result.seconds = time.monotonic() - start
        if check and not result.ok:
            raise CommandFailed(command, result)
        return result

    def spawn(self, command, stdout, stderr):
        if not command.background:
            raise ValueError(f'{command.template} is not a background command')
        self.log('$ ' + command.display() + ' &')
        return subprocess.Popen(command.argv, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                env={**os.environ, **command.env})

    @staticmethod
    def _plain(command):
        stdin = open(command.stdin_file, 'rb') if command.stdin_file else subprocess.DEVNULL
        try:
            done = subprocess.run(command.argv, stdin=stdin, capture_output=True, timeout=command.timeout,
                                  env={**os.environ, **command.env})
        except subprocess.TimeoutExpired as error:
            return Result(124, _text(error.stdout), f'timed out after {command.timeout:.0f} s')
        finally:
            if command.stdin_file:
                stdin.close()
        return Result(done.returncode, _text(done.stdout), _text(done.stderr))

    @staticmethod
    def _held(command):
        """Keep stdin open (the TV's Luna client replies only then); stop at `until`, `fail`, exit or timeout."""
        process = subprocess.Popen(command.argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE)
        out, err = [], []
        readers = [threading.Thread(target=_drain, args=(process.stdout, out), daemon=True),
                   threading.Thread(target=_drain, args=(process.stderr, err), daemon=True)]
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
            pipe.close()
        if code is None:
            return Result(124, ''.join(out), ''.join(err) + f'\ntimed out after {command.timeout:.0f} s')
        if command.until and code == 0 and not re.search(command.until, ''.join(out)):
            code = 1
        return Result(code, ''.join(out), ''.join(err))


def _drain(stream, sink):
    for line in iter(stream.readline, b''):
        sink.append(line.decode(errors='replace'))


def _text(data):
    return data.decode(errors='replace') if isinstance(data, bytes) else (data or '')


class Refusing:
    """The dry run's transport: executing anything is a bug."""

    def run(self, command, check=True):
        raise RuntimeError(f'dry run: refused to execute {command.template}')

    def spawn(self, command, stdout, stderr):
        raise RuntimeError(f'dry run: refused to start {command.template}')


class Transcript:
    """A recorded exchange for tests: each call must match the next entry's
    template; its result is returned and the command recorded."""

    def __init__(self, entries):
        self.entries = list(entries)
        self.calls = []

    def run(self, command, check=True):
        self.calls.append(command)
        if not self.entries:
            raise AssertionError(f'unexpected command {command.template}')
        template, result = self.entries.pop(0)
        if template != command.template:
            raise AssertionError(f'expected {template}, got {command.template}')
        if check and not result.ok:
            raise CommandFailed(command, result)
        return result

    def spawn(self, command, stdout, stderr):
        raise AssertionError(f'unexpected background command {command.template}')
