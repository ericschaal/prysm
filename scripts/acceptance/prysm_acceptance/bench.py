"""The pytest plugin that runs the cells (`acceptance host`, `tv` and `all`, docs/developers/acceptance.md).

A cell is the `cell` parameter of a cell module's `CELLS`; its fixtures and
checks are in that module (`host_cells`, `tv_cells`). This plugin adds what
every run shares: the configuration, a run directory with each cell's bench
(commands, logs, evidence), SIGTERM and SIGHUP ending a run like Ctrl-C so
the fixtures' teardowns restore the devices, the failure report (evidence,
log paths and the log's end), the warnings a cell leaves for the end of the
run (`warn_at_end`: a Prysm it removed), and `--dry-run`, which prints each
selected cell's fixtures and checks with their commands and actions from
`planned` and runs nothing.

A cell named with `--cells` fails when its artifact or tool is missing
(`unavailable`); any other cell is a skip that says why.
"""

from collections.abc import Callable, Generator, Iterator
import contextlib
from dataclasses import dataclass
import datetime
import inspect
import json
from pathlib import Path
import signal
from types import FrameType, ModuleType
from typing import Any, Generic, NoReturn, TypeVar, cast

import pytest

from prysm_acceptance import commands, transport
from prysm_acceptance import config as configuration
from prysm_acceptance.checks import Bench, Failed
from prysm_acceptance.config import Config
from prysm_acceptance.host import Sender
from prysm_acceptance.options import Cell, Delivery, Flavour, Platform, Source, TvCell

# The runner's project: scripts/acceptance, where config.toml lives.
HERE = Path(__file__).resolve().parents[1]
RUN_DIR = pytest.StashKey[Path]()
CONFIG = pytest.StashKey[Config]()
WARNINGS = pytest.StashKey[list[str]]()
SIGNALS = (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)
# A host cell or a TV cell; each cell module's fixtures and checks take its own kind.
AnyCell = Cell | TvCell
Kind = TypeVar('Kind', Cell, TvCell)


@dataclass(frozen=True)
class Planned:
    """A fixture's or check's description (its docstring) and what it does and undoes, as lines for a cell
    (of its module's kind) and the configuration."""

    description: str
    does: Callable[[Any, Config], list[str]]
    undoes: Callable[[Any, Config], list[str]]


# Filled by `planned` as the cell modules are imported, by module and in its order: fixtures first, then checks.
PLANS: dict[str, dict[str, Planned]] = {}
Function = TypeVar('Function', bound=Callable[..., object])


def _none(cell: object, config: Config) -> list[str]:
    return []


class Planner(Generic[Kind]):
    """`planned` for a cell module's own kind of cell: `planned = Planner[Cell]()`."""

    def __call__(
        self, does: Callable[[Kind, Config], list[str]] = _none, undoes: Callable[[Kind, Config], list[str]] = _none
    ) -> Callable[[Function], Function]:
        """Records a fixture's or check's docstring and commands and actions for the dry run.

        Put it under `@pytest.fixture`, so it sees the function itself."""

        def record(function: Function) -> Function:
            description = inspect.getdoc(function) or function.__name__
            PLANS.setdefault(function.__module__, {})[function.__name__] = Planned(description, does, undoes)
            return function

        return record


def only(*parts: Delivery | Flavour) -> pytest.MarkDecorator:
    """A check of what these deliveries or TV packages add: it is not collected for the other cells."""
    return pytest.mark.only(*parts)


def pytest_collection_modifyitems(config: pytest.Config, items: list[pytest.Item]) -> None:
    """Deselect a delivery's or package's own checks (`only`) for the other cells."""
    kept: list[pytest.Item] = []
    dropped: list[pytest.Item] = []
    for item in items:
        mark = item.get_closest_marker('only')
        cell = None
        if isinstance(item, pytest.Function) and hasattr(item, 'callspec'):
            cell = cast('AnyCell | None', item.callspec.params.get('cell'))
        mine = mark is None or cell is None or any(part in cell.id.split('-') for part in mark.args)
        (kept if mine else dropped).append(item)
    if dropped:
        config.hook.pytest_deselected(items=dropped)
        items[:] = kept


def pytest_addoption(parser: pytest.Parser) -> None:
    group = parser.getgroup('prysm acceptance')
    group.addoption('--bench-config', default=str(HERE / 'config.toml'), help='the bench configuration (TOML)')
    group.addoption('--wled', help='WLED host[:port], overrides the configuration')
    group.addoption('--cells', default='', help='only these cells (IDs, comma-separated); a missing artifact fails')
    group.addoption('--dry-run', action='store_true', help='print what each cell would do; run and contact nothing')
    group.addoption('--clip', help='file source: this clip instead of the generated one (colour check: not black)')
    group.addoption('--display-check', action='store_true', help="device source: colour fields on this Mac's display")
    group.addoption('--tv-is-free', action='store_true', help='the TV cells run (the owner said the TV may be used)')
    group.addoption('--runs', default=str(commands.ROOT / 'work' / 'acceptance'), help='where run directories go')
    group.addoption('--stage', default='host', help='the run directory is named TIME-STAGE')


@pytest.hookimpl(tryfirst=True)
def pytest_configure(config: pytest.Config) -> None:
    path = Path(str(config.getoption('bench_config')))
    dry_run = bool(config.getoption('dry_run'))
    if not path.exists() and dry_run:
        path = HERE / 'config.example.toml'
    try:
        if not path.exists():
            raise configuration.ConfigError(f'{path} does not exist: copy config.example.toml and fill it in')
        values = configuration.load(path)
        wled = config.getoption('wled')
        if wled:
            values.wled = configuration.address(str(wled), '--wled')
    except configuration.ConfigError as error:
        raise pytest.UsageError(f'config: {error}') from error
    config.stash[CONFIG] = values
    config.stash[WARNINGS] = []
    config.addinivalue_line('markers', 'tv: the TV cells')
    for kind in (Platform, Delivery, Source, Flavour):
        for member in kind:
            config.addinivalue_line('markers', f'{member}: the cells of {kind.__name__.lower()} {member}')
    if dry_run:
        return
    stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S')
    run_dir = Path(str(config.getoption('runs'))) / f'{stamp}-{config.getoption("stage")}'
    run_dir.mkdir(parents=True)
    config.stash[RUN_DIR] = run_dir
    # The report: JUnit XML with each check's detail, unless --junitxml says where.
    config.option.xmlpath = config.option.xmlpath or str(run_dir / 'report.xml')
    for number in SIGNALS[1:]:
        signal.signal(number, _interrupt)


def _interrupt(signum: int, frame: FrameType | None) -> None:
    raise KeyboardInterrupt


@contextlib.contextmanager
def uninterrupted() -> Generator[None, None, None]:
    """Restoring a device is not cut short by another Ctrl-C, SIGTERM or SIGHUP."""
    previous = {number: signal.signal(number, signal.SIG_IGN) for number in SIGNALS}
    try:
        yield
    finally:
        for number, handler in previous.items():
            signal.signal(number, handler)


def pytest_report_header(config: pytest.Config) -> str:
    if RUN_DIR not in config.stash:
        return 'prysm acceptance: dry run, nothing runs and nothing is contacted'
    return f'prysm acceptance: run directory {config.stash[RUN_DIR]}'


def _named(config: pytest.Config) -> list[str]:
    return [name for name in str(config.getoption('cells')).split(',') if name]


def pytest_generate_tests(metafunc: pytest.Metafunc) -> None:
    """The collecting module's cells (a check one cell module takes from another runs with its own cells)."""
    if 'cell' not in metafunc.fixturenames:
        return
    module = cast('ModuleType', metafunc.module)  # pyright: ignore[reportUnknownMemberType] -- untyped in pytest
    cells = cast('list[AnyCell]', getattr(module, 'CELLS', []))
    if cells and isinstance(cells[0], TvCell) and not metafunc.config.getoption('tv_is_free'):
        cells = []  # only when the owner said the TV may be used now
    wanted = _named(metafunc.config)
    unknown = set(wanted) - {cell.id for cell in cells}
    if unknown:
        raise pytest.UsageError(f'--cells: no cell {", ".join(sorted(unknown))} (cells: {[c.id for c in cells]})')
    chosen = [cell for cell in cells if not wanted or cell.id in wanted]
    params = [
        pytest.param(cell, id=cell.id, marks=[getattr(pytest.mark, part) for part in cell.id.split('-')])
        for cell in chosen
    ]
    metafunc.parametrize('cell', params, indirect=True, scope='module')


@pytest.fixture(scope='module')
def cell(request: pytest.FixtureRequest) -> AnyCell:
    return cast('AnyCell', request.param)


@pytest.fixture(scope='session')
def config(request: pytest.FixtureRequest) -> Config:
    return request.config.stash[CONFIG]


@pytest.fixture(scope='module')
def bench(cell: AnyCell, config: Config, request: pytest.FixtureRequest) -> Iterator[Bench]:
    """This cell's bench: the configuration, the commands' transport and log, its run directory."""
    out_dir = request.config.stash[RUN_DIR] / cell.id
    out_dir.mkdir()
    with (out_dir / 'commands.log').open('a') as log_file:

        def log(line: str) -> None:
            log_file.write(line + '\n')
            log_file.flush()

        yield Bench(config, transport.Real(log), out_dir, log)


def unavailable(request: pytest.FixtureRequest, cell: AnyCell, reason: str) -> NoReturn:
    """A missing artifact or tool: a failure for a cell named with --cells, else a skip."""
    if cell.id in _named(request.config):
        pytest.fail(f'{cell.id}: {reason}', pytrace=False)
    pytest.skip(f'{cell.id}: {reason}')


def warn_at_end(request: pytest.FixtureRequest, text: str) -> None:
    """`text` is printed at the end of the run, also after a failure, Ctrl-C, SIGTERM or SIGHUP."""
    request.config.stash[WARNINGS].append(text)


def pytest_terminal_summary(terminalreporter: pytest.TerminalReporter, config: pytest.Config) -> None:
    for text in config.stash.get(WARNINGS, list[str]()):
        terminalreporter.write_sep('!', 'WARNING', red=True, bold=True)
        terminalreporter.write_line(text, red=True, bold=True)


@pytest.fixture
def record(request: pytest.FixtureRequest) -> Callable[[str, object], None]:
    """Saves a passed check's detail and evidence (evidence/CHECK.json); `-v` shows the detail."""

    def save(detail: str, evidence: object = None) -> None:
        _evidence(request.config, request.node.name, {'detail': detail, 'evidence': evidence})
        request.node.user_properties.append(('detail', detail))

    return save


def _evidence(config: pytest.Config, name: str, value: object) -> None:
    path = config.stash[RUN_DIR] / 'evidence' / f'{name}.json'
    path.parent.mkdir(exist_ok=True)
    path.write_text(json.dumps(value, indent=2, default=str) + '\n')


def pytest_report_teststatus(report: pytest.TestReport) -> tuple[str, str, str] | None:
    detail = dict(report.user_properties).get('detail')
    if report.when == 'call' and report.passed and detail:
        return 'passed', '.', f'PASSED: {detail}'
    return None


@pytest.hookimpl(wrapper=True)
def pytest_runtest_makereport(
    item: pytest.Item, call: pytest.CallInfo[None]
) -> Generator[None, pytest.TestReport, pytest.TestReport]:
    """A failure's report has its evidence, where the logs are and the end of the host's log (a remote host's
    log and kernel USB lines fetched first)."""
    report = yield
    if not report.failed or RUN_DIR not in item.config.stash:
        return report
    error = call.excinfo.value if call.excinfo else None
    if isinstance(error, Failed):
        _evidence(item.config, item.name, {'failed': str(error), 'evidence': error.evidence})
        if error.evidence is not None:
            report.sections.append(('evidence', json.dumps(error.evidence, indent=2, default=str)))
    seam = item.funcargs.get('running') if isinstance(item, pytest.Function) else None
    if seam is not None:
        try:
            cast('Sender', seam).fetch()  # a remote host's log and kernel lines, the TV's log, as they are now
        except Exception as fetched:  # the report still shows what is here
            report.sections.append(('logs not fetched', f'{type(fetched).__name__}: {fetched}'))
    run_dir = item.config.stash[RUN_DIR]
    logs = sorted(run_dir.glob('*/*.log'))
    tails = [
        f'--- {path.relative_to(run_dir)} (end)\n' + ''.join(path.read_text(errors='replace').splitlines(True)[-20:])
        for path in logs
        if path.name != 'commands.log'
    ]
    report.sections.append(('logs', '\n'.join(map(str, logs)) + ''.join('\n' + tail for tail in tails)))
    return report


def pytest_runtestloop(session: pytest.Session) -> bool | None:
    """--dry-run: print each selected cell's plan and run nothing."""
    if not session.config.getoption('dry_run'):
        return None
    values = session.config.stash[CONFIG]
    reporter = cast('pytest.TerminalReporter', session.config.pluginmanager.get_plugin('terminalreporter'))
    by_cell: dict[AnyCell, list[pytest.Function]] = {}
    for item in session.items:
        if isinstance(item, pytest.Function):
            by_cell.setdefault(cast('AnyCell', item.callspec.params['cell']), []).append(item)
    for cell, items in by_cell.items():
        _print_plan(cell, values, items, reporter.write_line)
    return True


def _print_plan(cell: AnyCell, values: Config, items: list[pytest.Function], write: Callable[[str], None]) -> None:
    """The fixtures of the cell's module, then each check from the module that defines it."""
    used = {name for item in items for name in item.fixturenames}
    module = PLANS.get(cast('ModuleType', items[0].module).__name__, {})
    fixtures = [name for name in module if name in used]
    write(f'\nCell {cell.id} (dry run: nothing is executed, nothing is contacted)')
    write('\nSet up, in this order:')
    for name in fixtures:
        _print_one(write, name, module[name].description, module[name].does(cell, values))
    write('\nChecks, in this order:')
    for item in items:
        function = cast('Callable[..., object]', item.function)  # pyright: ignore[reportUnknownMemberType]
        plan = PLANS.get(function.__module__, {}).get(item.originalname)
        if plan:
            _print_one(write, item.originalname, plan.description, plan.does(cell, values))
    write('\nAt the end (also after a failure, Ctrl-C, SIGTERM or SIGHUP), in this order:')
    for name in reversed(fixtures):
        for line in module[name].undoes(cell, values):
            write(f'  - {line}')


def _print_one(write: Callable[[str], None], name: str, description: str, lines: list[str]) -> None:
    title, _, rest = description.partition('\n')
    write(f'\n  {name}: {title}')
    for text in rest.strip().splitlines():
        write(f'      {text}')
    for line in lines:
        write(f'      {line}')
