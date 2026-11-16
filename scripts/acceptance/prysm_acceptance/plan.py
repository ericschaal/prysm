"""Steps, their outcomes, the run (selection, dry run, cleanup) and the report.

A step is `setup` (always runs, its failure blocks the rest), `auto` (a
check the runner makes), `guided` (a person acts or answers) or `skip` (no
safe or documented mechanism; listed with its reason). `--only` and
`--from` select checks; setup and cleanup steps always run, so a selected
check finds what it needs, and cleanup always restores the devices.
"""
from dataclasses import dataclass, field
import datetime
import fnmatch
import json
from pathlib import Path
import signal
import sys
import time
import traceback

PASS, FAIL, SKIP, INFO = 'PASS', 'FAIL', 'SKIP', 'INFO'
MODES = ('setup', 'auto', 'guided', 'skip')


class Failed(Exception):
    """A check that did not hold; `evidence` goes into the report."""

    def __init__(self, message, evidence=None):
        super().__init__(message)
        self.evidence = evidence


class Skipped(Exception):
    def __init__(self, reason, evidence=None):
        super().__init__(reason)
        self.evidence = evidence


class Interrupted(BaseException):
    """SIGTERM, raised like Ctrl-C so cleanups run."""


@dataclass
class Step:
    name: str
    title: str
    how: str
    mode: str
    run: object = None
    commands: object = None  # callable() -> [Command] for the dry run
    actions: list = field(default_factory=list)  # HTTP calls and other actions, for the dry run
    group: str | None = None  # a setup step with a group runs only when a check of that group is selected

    def __post_init__(self):
        if self.mode not in MODES:
            raise ValueError(self.mode)


@dataclass
class Outcome:
    name: str
    title: str
    mode: str
    status: str
    detail: str = ''
    evidence: object = None
    seconds: float = 0.0


class Context:
    """What steps share: configuration, options, transport, output directory, cleanups and data."""

    def __init__(self, config, options, transport, out_dir, ask=None, log=print):
        self.config = config
        self.options = options
        self.transport = transport
        self.out_dir = out_dir
        self.data = {}
        self.cleanups = []
        self.ask = ask or ask_person
        self.log = log

    def run(self, command, check=True):
        return self.transport.run(command, check=check)

    def cleanup(self, name, title, action):
        """Run `action()` at the end, also on failure or interruption (last registered first)."""
        self.cleanups.append((name, title, action))

    def save(self, name, value):
        path = self.out_dir / name
        path.write_text(value if isinstance(value, str) else json.dumps(value, indent=2) + '\n')
        return str(path)


def ask_person(question, choices=('y', 'n', 'skip')):
    """One question; the answer and an optional note. Without a terminal: skip."""
    if not sys.stdin.isatty():
        return 'skip', 'no terminal to ask'
    while True:
        answer = input(f'\n>>> {question}\n    [{"/".join(choices)}]: ').strip().lower()
        if answer in choices:
            break
    note = input('    note (optional): ').strip()
    return answer, note


def selected(steps, only, start):
    """Names of the checks to run: all, `--only` patterns, or from `--from` on."""
    names = [s.name for s in steps if s.mode != 'setup']
    if only:
        patterns = [p for item in only for p in item.split(',') if p]
        return {n for n in names if any(fnmatch.fnmatch(n, p) for p in patterns)}
    if start:
        matches = [i for i, n in enumerate(names) if fnmatch.fnmatch(n, start)]
        if not matches:
            raise SystemExit(f'--from {start}: no such step')
        return set(names[matches[0]:])
    return set(names)


def print_plan(stage, steps, cleanups=(), cleanup_commands=(), log=print):
    log(f'Plan for stage {stage} (dry run: nothing is executed, nothing is contacted)')
    ssh = None
    for step in steps:
        log(f'\n[{step.mode:6}] {step.name}: {step.title}')
        log(f'         verified by: {step.how}')
        for action in step.actions:
            log(f'         - {action}')
        for command in (step.commands() if step.commands else []):
            if command.remote:
                ssh = ssh or ' '.join(command.argv[:-1])
                extra = (' < ' + command.stdin_file if command.stdin_file else '') + \
                    (f'   [stdin held open, {command.timeout:.0f} s]' if command.hold_stdin else '')
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
        log(f'\nTV$ commands run as: {ssh} \'COMMAND\' (each with its own timeout)')
    log('')


def execute(stage, steps, ctx, only=(), start=None):
    """Run the steps; cleanups at the end whatever happens. Returns the outcomes."""
    chosen = selected(steps, only, start)
    groups = {s.group for s in steps if s.mode != 'setup' and s.name in chosen}
    outcomes = []
    blocked = None
    previous = {number: signal.signal(number, _raise_interrupted) for number in (signal.SIGTERM, signal.SIGHUP)}
    try:
        for step in steps:
            if step.mode != 'setup' and step.name not in chosen:
                continue
            if step.mode == 'setup' and step.group and step.group not in groups:
                continue
            if blocked:
                outcomes.append(Outcome(step.name, step.title, step.mode, SKIP, f'blocked: {blocked} failed'))
                continue
            outcome = _one(step, ctx)
            outcomes.append(outcome)
            ctx.log(f'{outcome.status:4} {step.name}: {outcome.detail}')
            if outcome.status == FAIL and step.mode == 'setup':
                blocked = step.name
    except (KeyboardInterrupt, Interrupted):
        outcomes.append(Outcome('interrupted', 'Run interrupted', 'setup', FAIL, 'Ctrl-C or SIGTERM'))
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


def _raise_interrupted(signum, frame):
    raise Interrupted()


def _one(step, ctx):
    start = time.monotonic()
    if step.mode == 'skip':
        status, detail, evidence = SKIP, step.how, None
    else:
        try:
            result = step.run(ctx)
            status, detail, evidence = PASS, '', None
            if isinstance(result, tuple):
                detail, evidence = result
            elif isinstance(result, str):
                detail = result
            elif result is not None:
                evidence = result
            if step.mode == 'guided' and isinstance(evidence, dict) and evidence.get('answer'):
                status = {'y': PASS, 'n': FAIL}.get(evidence['answer'], SKIP)
        except Failed as error:
            status, detail, evidence = FAIL, str(error), error.evidence
        except Skipped as error:
            status, detail, evidence = SKIP, str(error), error.evidence
        except (KeyboardInterrupt, Interrupted):
            raise
        except Exception as error:  # a broken step is a failure, with its cause
            status, detail = FAIL, f'{type(error).__name__}: {error}'
            evidence = traceback.format_exc(limit=4)
    return Outcome(step.name, step.title, step.mode, status, detail, evidence,
                   round(time.monotonic() - start, 1))


def _cleanups(ctx):
    found = []
    while ctx.cleanups:
        name, title, action = ctx.cleanups.pop()
        try:
            result = action()
            found.append(Outcome(name, title, 'setup', PASS, result if isinstance(result, str) else ''))
        except Exception as error:
            found.append(Outcome(name, title, 'setup', FAIL, f'{type(error).__name__}: {error}'))
        ctx.log(f'{found[-1].status:4} {name}: {found[-1].detail}')
    return found


def output_directory(root, stage):
    stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S')
    path = Path(root) / 'work' / 'acceptance' / f'{stamp}-{stage}'
    path.mkdir(parents=True, exist_ok=False)
    return path


def write_report(out_dir, stage, outcomes, facts):
    failed = [o for o in outcomes if o.status == FAIL]
    data = {'stage': stage, 'facts': facts, 'passed': not failed,
            'outcomes': [o.__dict__ for o in outcomes]}
    (out_dir / 'report.json').write_text(json.dumps(data, indent=2, default=str) + '\n')
    lines = [f'# Prysm acceptance: {stage}', '']
    lines += [f'- {key}: {value}' for key, value in facts.items()]
    lines += [f'- Result: {"FAILED" if failed else "passed"} '
              f'({sum(o.status == PASS for o in outcomes)} passed, {len(failed)} failed, '
              f'{sum(o.status == SKIP for o in outcomes)} skipped)', '',
              '| Step | Kind | Result | Detail |', '| --- | --- | --- | --- |']
    for o in outcomes:
        detail = str(o.detail).replace('|', '\\|').replace('\n', ' ')
        lines.append(f'| `{o.name}` | {o.mode} | {o.status} | {detail} |')
    skipped = [o for o in outcomes if o.status == SKIP]
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
