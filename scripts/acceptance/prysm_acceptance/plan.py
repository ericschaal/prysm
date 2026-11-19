"""The `eyes` stage's step runner: steps, outcomes, selection, dry run, cleanup, report.

A step is `setup` (always runs, its failure blocks the rest), `auto` (a
check the runner makes), `guided` (a person acts or answers) or `skip` (no
safe or documented mechanism; listed with its reason). `--only` and
`--from` select checks; setup and cleanup steps always run, so a selected
check finds what it needs, and cleanup always restores the devices.
"""

from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass, field
import datetime
from enum import StrEnum
import fnmatch
import json
from pathlib import Path
import signal
import time
import traceback
from types import FrameType
from typing import Generic, TypeVar, assert_never, cast

from prysm_acceptance import checks
from prysm_acceptance.checks import Answer, Ask, Failed, Skipped, ask_person
from prysm_acceptance.commands import Command
from prysm_acceptance.config import Config
from prysm_acceptance.transport import Transport


class Status(StrEnum):
    """How a step ended."""

    PASS = 'PASS'  # noqa: S105 -- a step's status, not a password
    FAIL = 'FAIL'
    SKIP = 'SKIP'


class Mode(StrEnum):
    """What kind of step it is (see the module's description)."""

    SETUP = 'setup'
    AUTO = 'auto'
    GUIDED = 'guided'
    SKIP = 'skip'


class Interrupted(BaseException):
    """SIGTERM, raised like Ctrl-C so cleanups run."""


class Bench(checks.Bench):
    """What every step and helper of an `eyes` run uses: the shared bench, the report's facts and the person
    to ask."""

    def __init__(
        self,
        config: Config,
        transport: Transport,
        out_dir: Path,
        ask: Ask | None = None,
        log: Callable[[str], None] = print,
    ) -> None:
        super().__init__(config, transport, out_dir, log)
        self.facts: dict[str, str] = {}
        self.ask = ask or ask_person


# What a stage's steps pass on to later ones: a TypedDict per stage (`stage_data`).
Data = TypeVar('Data')


class Context(Bench, Generic[Data]):
    """A stage's bench and the data its steps pass on (`data`, filled as the steps run)."""

    def __init__(
        self,
        config: Config,
        transport: Transport,
        out_dir: Path,
        ask: Ask | None = None,
        log: Callable[[str], None] = print,
    ) -> None:
        super().__init__(config, transport, out_dir, ask, log)
        # Empty: every key of a stage's data is filled by the step that provides it.
        self.data = cast('Data', {})


@dataclass
class Step(Generic[Data]):
    name: str
    title: str
    how: str
    mode: Mode
    run: Callable[[Context[Data]], object] | None = None
    commands: Callable[[], Iterable[Command]] | None = None  # for the dry run
    actions: list[str] = field(default_factory=list[str])  # HTTP calls and other actions, for the dry run

    def __post_init__(self) -> None:
        self.mode = Mode(self.mode)


@dataclass
class Outcome:
    name: str
    title: str
    mode: Mode
    status: Status
    detail: object = ''
    evidence: object = None
    seconds: float = 0.0


def selected(steps: Sequence[Step[Data]], only: Sequence[str], start: str | None) -> set[str]:
    """Names of the checks to run: all, `--only` patterns, or from `--from` on."""
    names = [s.name for s in steps if s.mode != Mode.SETUP]
    if only:
        patterns = [p for item in only for p in item.split(',') if p]
        return {n for n in names if any(fnmatch.fnmatch(n, p) for p in patterns)}
    if start:
        matches = [i for i, n in enumerate(names) if fnmatch.fnmatch(n, start)]
        if not matches:
            raise SystemExit(f'--from {start}: no such step')
        return set(names[matches[0] :])
    return set(names)


def print_plan(
    stage: str,
    steps: Sequence[Step[Data]],
    cleanups: Sequence[str] = (),
    cleanup_commands: Sequence[Command] = (),
    log: Callable[[str], None] = print,
) -> None:
    log(f'Plan for stage {stage} (dry run: nothing is executed, nothing is contacted)')
    ssh = None
    for step in steps:
        log(f'\n[{step.mode:6}] {step.name}: {step.title}')
        log(f'         verified by: {step.how}')
        for action in step.actions:
            log(f'         - {action}')
        listed: Iterable[Command] = step.commands() if step.commands else []
        for command in listed:
            if command.remote:
                ssh = ssh or ' '.join(command.argv[:-1])
                extra = (' < ' + command.stdin_file if command.stdin_file else '') + (
                    f'   [stdin held open, {command.timeout:.0f} s]' if command.hold_stdin else ''
                )
                log(f'         TV$ {command.remote}{extra}')
            else:
                log(f'         $ {command.display()}')
    if cleanups:
        log('\nAlways at the end (also after a failure or Ctrl-C), in this order:')
        for line in cleanups:
            log(f'  - {line}')
        for command in cleanup_commands:
            if command.remote:
                ssh = ssh or ' '.join(command.argv[:-1])
                log(f'    TV$ {command.remote}')
            else:
                log(f'    $ {command.display()}')
    if ssh:
        log(f"\nTV$ commands run as: {ssh} 'COMMAND' (each with its own timeout)")
    log('')


def execute(
    stage: str, steps: Sequence[Step[Data]], ctx: Context[Data], only: Sequence[str] = (), start: str | None = None
) -> list[Outcome]:
    """Run the steps; cleanups at the end whatever happens. Returns the outcomes."""
    chosen = selected(steps, only, start)
    outcomes: list[Outcome] = []
    blocked = None
    previous = {number: signal.signal(number, _raise_interrupted) for number in (signal.SIGTERM, signal.SIGHUP)}
    try:
        for step in steps:
            if step.mode != Mode.SETUP and step.name not in chosen:
                continue
            if blocked:
                outcomes.append(Outcome(step.name, step.title, step.mode, Status.SKIP, f'blocked: {blocked} failed'))
                continue
            outcome = _one(step, ctx)
            outcomes.append(outcome)
            ctx.log(f'{outcome.status:4} {step.name}: {outcome.detail}')
            if outcome.status == Status.FAIL and step.mode == Mode.SETUP:
                blocked = step.name
    except (KeyboardInterrupt, Interrupted):
        outcomes.append(Outcome('interrupted', 'Run interrupted', Mode.SETUP, Status.FAIL, 'Ctrl-C or SIGTERM'))
        ctx.log('Interrupted: cleaning up')
    finally:
        # Restoring the devices must not be cut short by another Ctrl-C or SIGTERM.
        interrupt = signal.signal(signal.SIGINT, signal.SIG_IGN)
        for number in previous:
            signal.signal(number, signal.SIG_IGN)
        outcomes += _cleanups(ctx)
        signal.signal(signal.SIGINT, interrupt)
        for number, handler in previous.items():
            signal.signal(number, handler)
    return outcomes


def _raise_interrupted(signum: int, frame: FrameType | None) -> None:
    raise Interrupted


def _one(step: Step[Data], ctx: Context[Data]) -> Outcome:
    start = time.monotonic()
    status = Status.PASS
    detail: object = ''
    evidence: object = None
    match step.mode:
        case Mode.SKIP:
            status, detail = Status.SKIP, step.how
        case Mode.SETUP | Mode.AUTO | Mode.GUIDED:
            try:
                # A step that is not a skip has a `run`; it returns a detail, (detail, evidence) or evidence.
                result = cast('Callable[[Context[Data]], object]', step.run)(ctx)
                if isinstance(result, tuple):
                    detail, evidence = cast('tuple[object, object]', result)
                elif isinstance(result, str):
                    detail = result
                elif result is not None:
                    evidence = result
                if step.mode == Mode.GUIDED:
                    status = _answered(evidence)
            except Failed as error:
                status, detail, evidence = Status.FAIL, str(error), error.evidence
            except Skipped as error:
                status, detail, evidence = Status.SKIP, str(error), error.evidence
            except (KeyboardInterrupt, Interrupted):
                raise
            except Exception as error:  # a broken step is a failure, with its cause
                status, detail = Status.FAIL, f'{type(error).__name__}: {error}'
                evidence = traceback.format_exc(limit=4)
        case _:
            assert_never(step.mode)
    return Outcome(step.name, step.title, step.mode, status, detail, evidence, round(time.monotonic() - start, 1))


def _answered(evidence: object) -> Status:
    """A guided step's status from the person's answer in its evidence (PASS without one)."""
    answer = cast('dict[str, object]', evidence).get('answer') if isinstance(evidence, dict) else None
    if not answer:
        return Status.PASS
    match answer:
        case Answer.YES:
            return Status.PASS
        case Answer.NO:
            return Status.FAIL
        case _:
            return Status.SKIP


def _cleanups(ctx: Bench) -> list[Outcome]:
    found: list[Outcome] = []
    while ctx.cleanups:
        name, title, action = ctx.cleanups.pop()
        try:
            result = action()
            found.append(Outcome(name, title, Mode.SETUP, Status.PASS, result if isinstance(result, str) else ''))
        except Exception as error:
            found.append(Outcome(name, title, Mode.SETUP, Status.FAIL, f'{type(error).__name__}: {error}'))
        ctx.log(f'{found[-1].status:4} {name}: {found[-1].detail}')
    return found


def output_directory(root: Path, stage: str) -> Path:
    stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S')
    path = Path(root) / 'work' / 'acceptance' / f'{stamp}-{stage}'
    path.mkdir(parents=True, exist_ok=False)
    return path


def write_report(out_dir: Path, stage: str, outcomes: Sequence[Outcome], facts: dict[str, str]) -> bool:
    failed = [o for o in outcomes if o.status == Status.FAIL]
    data = {'stage': stage, 'facts': facts, 'passed': not failed, 'outcomes': [o.__dict__ for o in outcomes]}
    (out_dir / 'report.json').write_text(json.dumps(data, indent=2, default=str) + '\n')
    lines = [f'# Prysm acceptance: {stage}', '']
    lines += [f'- {key}: {value}' for key, value in facts.items()]
    lines += [
        f'- Result: {"FAILED" if failed else "passed"} '
        f'({sum(o.status == Status.PASS for o in outcomes)} passed, {len(failed)} failed, '
        f'{sum(o.status == Status.SKIP for o in outcomes)} skipped)',
        '',
        '| Step | Kind | Result | Detail |',
        '| --- | --- | --- | --- |',
    ]
    for o in outcomes:
        detail = str(o.detail).replace('|', '\\|').replace('\n', ' ')
        lines.append(f'| `{o.name}` | {o.mode} | {o.status} | {detail} |')
    skipped = [o for o in outcomes if o.status == Status.SKIP]
    if skipped:
        lines += ['', '## Not automated or not run', '']
        lines += [f'- `{o.name}`: {o.detail}' for o in skipped]
    lines += ['', '## Evidence', '']
    for o in outcomes:
        if o.evidence is not None:
            text = o.evidence if isinstance(o.evidence, str) else json.dumps(o.evidence, indent=2, default=str)
            lines += [f'### `{o.name}`', '', '```text', text.strip(), '```', '']
    (out_dir / 'report.md').write_text('\n'.join(lines) + '\n')
    return not failed
