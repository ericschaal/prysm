"""What a check raises, and the bench a run's helpers share.

A check that does not hold raises `Failed` with its evidence; one that cannot
run here raises `Skipped`. `Bench` runs allow-listed commands, registers what
the end of the run undoes (last registered first) and saves evidence.
"""

from collections.abc import Callable
from enum import StrEnum
import json
from pathlib import Path
import sys
from typing import Protocol

from prysm_acceptance.commands import Command
from prysm_acceptance.config import Config
from prysm_acceptance.transport import Result, Transport


class Failed(Exception):
    """A check that did not hold; `evidence` goes into the report."""

    def __init__(self, message: str, evidence: object = None) -> None:
        super().__init__(message)
        self.evidence = evidence


class Skipped(Exception):
    def __init__(self, reason: str, evidence: object = None) -> None:
        super().__init__(reason)
        self.evidence = evidence


class Answer(StrEnum):
    """A person's answer to a guided question."""

    YES = 'y'
    NO = 'n'
    SKIP = 'skip'


ALL_ANSWERS = (Answer.YES, Answer.NO, Answer.SKIP)


class Ask(Protocol):
    """Asks a person one question; the answer and an optional note."""

    def __call__(self, question: str, choices: tuple[Answer, ...] = ALL_ANSWERS) -> tuple[Answer, str]: ...


def ask_person(question: str, choices: tuple[Answer, ...] = ALL_ANSWERS) -> tuple[Answer, str]:
    """One question; the answer and an optional note. Without a terminal, or at its end of input: skip."""
    if not sys.stdin.isatty():
        return Answer.SKIP, 'no terminal to ask'
    try:
        while True:
            answer = input(f'\n>>> {question}\n    [{"/".join(choices)}]: ').strip().lower()
            if answer in choices:
                break
        note = input('    note (optional, Enter for none): ').strip()
    except EOFError:
        return Answer.SKIP, 'no answer (end of input)'
    return Answer(answer), note


def tell_person(text: str) -> None:
    """One line for the person at the terminal, so a wait after their answer is not silent; nothing without a
    terminal."""
    if sys.stdin.isatty():
        print(f'    ... {text}', flush=True)


# One undo: its name, its title and what it does (a detail, or anything).
Undo = tuple[str, str, Callable[[], object]]


class Bench:
    """The configuration, the transport that runs commands, the run directory and the undos."""

    def __init__(self, config: Config, transport: Transport, out_dir: Path, log: Callable[[str], None] = print) -> None:
        self.config = config
        self.transport = transport
        self.out_dir = out_dir
        self.log = log
        self.cleanups: list[Undo] = []

    def run(self, command: Command, check: bool = True) -> Result:
        return self.transport.run(command, check=check)

    def cleanup(self, name: str, title: str, action: Callable[[], object]) -> None:
        """Run `action()` at the end, also on failure or interruption (last registered first)."""
        self.cleanups.append((name, title, action))

    def undo(self) -> list[str]:
        """Run the registered cleanups, last first, each also after one failed; the failures."""
        failures: list[str] = []
        while self.cleanups:
            name, _, action = self.cleanups.pop()
            try:
                action()
            except Exception as error:  # every cleanup runs; the caller reports what failed
                failures.append(f'{name}: {type(error).__name__}: {error}')
        return failures

    def save(self, name: str, value: object) -> str:
        path = self.out_dir / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value if isinstance(value, str) else json.dumps(value, indent=2, default=str) + '\n')
        return str(path)
