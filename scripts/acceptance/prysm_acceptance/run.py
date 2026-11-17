"""Prysm hardware acceptance runner (docs/developers/acceptance.md).

  acceptance host mac|linux [--delivery binary|deb|image] [--source file|file:PATH|device] [--display-check]
                            [--prepare] [PYTEST ARGS]
  acceptance tv full --tv-is-free [--prepare] [PYTEST ARGS]
  acceptance tv slim --receiver mac|linux --tv-is-free [--prepare] [PYTEST ARGS]
  acceptance all [--tv-is-free] [--prepare] [PYTEST ARGS]
  acceptance eyes [--portal URL]
  acceptance restore-wled SNAPSHOT.json
  acceptance settings restore tv|linux BACKUP.json

Each takes --config FILE (default scripts/acceptance/config.toml) and
--dry-run (print what would run; execute and contact nothing). `host` and
`tv` run their cells with pytest: other arguments go to pytest (-k, -m, -x,
-v ...); naming the source, the deb or image delivery, or a TV package names
the cell, which then fails instead of skipping when its artifact or tool is
missing. A TV cell runs only with --tv-is-free, the owner's word that the TV
may be used now; `tv` refuses without it. `all` runs every host cell, one at
a time, and with --tv-is-free the TV cells after them, each skipped when its
artifact, tool or device is missing. --prepare first builds the chosen
cells' configured artifacts with `cargo xtask dist`. `eyes` takes --only
NAME[,NAME] and --from NAME (patterns over step names; setup and cleanup
always run). `settings restore` sends a backup a cell took before it removed
Prysm (`installed = "replace"`) to the Prysm reinstalled on that target.
Reports go to work/acceptance/<time>-<stage>/.
Exit code: 0 all passed (skips allowed), 1 a failure, 2 bad usage, 3 the
preparation (--prepare) failed and nothing was contacted.
"""

import argparse
from collections.abc import Sequence
from dataclasses import dataclass, field
from enum import StrEnum
import json
from pathlib import Path
import sys
from typing import cast

import pytest

from prysm_acceptance import bench, commands, host_deb, installed, plan, prepare, stage_eyes, transport, tv, wled_api
from prysm_acceptance import config as configuration
from prysm_acceptance.checks import Failed
from prysm_acceptance.config import SSH, Config
from prysm_acceptance.host import shown
from prysm_acceptance.options import CELLS, TV_CELLS, Cell, Delivery, Flavour, Platform, Source, TvCell
from prysm_acceptance.stage_eyes import EyesData

# The runner's project: scripts/acceptance, where config.toml lives.
HERE = Path(__file__).resolve().parents[1]
HOST_CELLS = Path(__file__).with_name('host_cells.py')
TV_CELLS_MODULE = Path(__file__).with_name('tv_cells.py')
# The exit code of a failed --prepare.
PREPARATION_FAILED = 3


class Stage(StrEnum):
    HOST = 'host'
    TV = 'tv'
    ALL = 'all'
    EYES = 'eyes'
    RESTORE_WLED = 'restore-wled'
    SETTINGS = 'settings'


@dataclass(frozen=True)
class Arguments:
    """The parsed command line; each stage reads its own."""

    stage: Stage
    config: str
    dry_run: bool
    wled: str | None = None
    # host, tv and all
    platform: Platform = Platform.MAC
    delivery: Delivery = Delivery.BINARY
    source: Source | None = None
    clip: str | None = None
    display_check: bool = False
    flavour: Flavour | None = None
    receiver: Platform | None = None
    tv_is_free: bool = False
    prepare: bool = False
    pytest: list[str] = field(default_factory=list[str])
    # eyes
    only: list[str] = field(default_factory=list[str])
    start: str | None = None
    portal: str | None = None
    # restore-wled
    snapshot: str | None = None
    # settings restore
    action: str | None = None
    target: str | None = None
    backup: str | None = None


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument('--config', default=str(HERE / 'config.toml'))
    common.add_argument('--dry-run', action='store_true')
    common.add_argument('--wled', help='WLED host[:port], overrides the config')
    stages = parser.add_subparsers(dest='stage', required=True)
    building = argparse.ArgumentParser(add_help=False)
    building.add_argument('--prepare', action='store_true', help='build the artifacts first (cargo xtask dist)')
    free = argparse.ArgumentParser(add_help=False)
    free.add_argument('--tv-is-free', action='store_true', help='the owner said the TV may be used now')
    host = stages.add_parser(Stage.HOST, parents=[common, building], help='the host cells (pytest)')
    host.add_argument('platform', choices=[platform.value for platform in Platform], help='where Prysm runs')
    host.add_argument('--delivery', choices=[delivery.value for delivery in Delivery], default=Delivery.BINARY)
    host.add_argument('--source', help='file, file:PATH or device (default: every source of the delivery)')
    host.add_argument('--display-check', action='store_true', help="device: colour fields on this computer's display")
    tv_stage = stages.add_parser(Stage.TV, parents=[common, building, free], help='one TV cell (pytest)')
    tv_stage.add_argument('flavour', choices=[flavour.value for flavour in Flavour], help='the TV package')
    tv_stage.add_argument('--receiver', choices=[platform.value for platform in Platform], help='slim: where it runs')
    stages.add_parser(Stage.ALL, parents=[common, building, free], help='every cell, one at a time (pytest)')
    eyes = stages.add_parser(Stage.EYES, parents=[common], help='what a person judges')
    eyes.add_argument('--only', action='append', default=[])
    eyes.add_argument('--from', dest='start')
    eyes.add_argument('--portal', help='the Prysm portal to set states on')
    restore = stages.add_parser(Stage.RESTORE_WLED, parents=[common], help='put WLED back from a snapshot')
    restore.add_argument('snapshot', help='the wled-snapshot.json of a run')
    settings = stages.add_parser(Stage.SETTINGS, parents=[common], help="a removed Prysm's backed-up settings")
    settings.add_argument('action', choices=['restore'])
    settings.add_argument('target', choices=['tv', 'linux'], help='the configured target the backup was taken on')
    settings.add_argument('backup', help=f'the {installed.BACKUP} of a run')
    return parser


def arguments(argv: Sequence[str]) -> Arguments:
    parser = _parser()
    values, rest = parser.parse_known_args(argv)
    found = vars(values)
    stage = Stage(found.pop('stage'))
    if rest and stage not in (Stage.HOST, Stage.TV, Stage.ALL):
        parser.error(f'unrecognized arguments: {" ".join(rest)}')
    if stage in (Stage.TV, Stage.ALL):
        found['pytest'] = rest
    if stage == Stage.TV:
        flavour, receiver = Flavour(found['flavour']), found['receiver']
        if (flavour == Flavour.SLIM) != (receiver is not None):
            parser.error('tv slim takes --receiver mac|linux; tv full takes none')
        if not found['tv_is_free']:
            parser.error('tv: pass --tv-is-free once the owner has said the TV may be used now')
        found |= {'flavour': flavour, 'receiver': Platform(receiver) if receiver else None}
    if stage == Stage.HOST:
        source = cast('str | None', found.pop('source'))
        if source and source.startswith('file:'):
            found['clip'], source = source[5:], Source.FILE
        if source and source not in tuple(Source):
            parser.error('--source is file, file:PATH or device')
        cells = [cell for cell in CELLS if (cell.platform, cell.delivery) == (found['platform'], found['delivery'])]
        if not cells:
            parser.error(f'{found["platform"]} has no {found["delivery"]} cell')
        if len(cells) == 1 and source and source != cells[0].source:
            parser.error(f'the {found["delivery"]} delivery has only the {cells[0].source} source')
        found |= {
            # A delivery with one cell names it, as --source does.
            'source': Source(source) if source else (cells[0].source if len(cells) == 1 else None),
            'platform': Platform(found['platform']),
            'delivery': Delivery(found['delivery']),
            'pytest': rest,
        }
    return Arguments(stage=stage, **found)


def load_config(args: Arguments) -> Config:
    path = Path(args.config)
    if path.exists():
        config = configuration.load(path)
    elif args.dry_run:
        config = configuration.load(HERE / 'config.example.toml')
    else:
        raise configuration.ConfigError(f'{path} does not exist: copy config.example.toml and fill it in')
    if args.wled:
        config.wled = configuration.address(args.wled, '--wled')
    if args.portal:
        config.eyes_portal = args.portal
    return config


def chosen(args: Arguments) -> list[Cell | TvCell]:
    """The cells a `host`, `tv` or `all` run may run (before pytest's own selection)."""
    match args.stage:
        case Stage.ALL:
            return [*CELLS, *(TV_CELLS if args.tv_is_free else ())]
        case Stage.TV:
            return [TvCell(cast('Flavour', args.flavour), args.receiver)]
        case _:
            return [
                cell
                for cell in CELLS
                if (cell.platform, cell.delivery) == (args.platform, args.delivery)
                and (args.source is None or cell.source == args.source)
            ]


def prepared(args: Arguments) -> int | None:
    """--prepare: the build command for the chosen cells' artifacts, printed (dry run) or run; the exit code
    when it failed, None to go on."""
    try:
        config = load_config(args)
    except configuration.ConfigError as error:
        print(f'config: {error}', file=sys.stderr)
        return 2
    try:
        build = prepare.command(chosen(args), config)
    except prepare.PrepareError as error:
        print(f'--prepare: {error}', file=sys.stderr)
        return PREPARATION_FAILED
    if build is None:
        print('--prepare: no artifact configured for the chosen cells; nothing to build')
        return None
    if args.dry_run:
        print(f'--prepare (dry run, not run): $ {build.display()}')
        return None
    print(f'--prepare: $ {build.display()}', flush=True)
    done = transport.Real(lambda line: None).run(build, check=False)
    if not done.ok:
        print(done.stdout[-3000:] + done.stderr[-3000:], file=sys.stderr)
        print(f'--prepare: the build exited {done.returncode}; nothing was contacted', file=sys.stderr)
        return PREPARATION_FAILED
    return None


def cells(args: Arguments) -> int:
    """The host and TV cells under pytest, with the `bench` plugin; pytest's exit code as the runner's."""
    if args.prepare:
        failed = prepared(args)
        if failed is not None:
            return failed
    with_tv = args.stage == Stage.TV or (args.stage == Stage.ALL and args.tv_is_free)
    modules = {Stage.HOST: [HOST_CELLS], Stage.TV: [TV_CELLS_MODULE]}.get(
        args.stage, [HOST_CELLS, *([TV_CELLS_MODULE] if with_tv else [])]
    )
    named = [cell.id for cell in chosen(args)] if args.stage == Stage.TV or args.source else []
    options = [
        *map(str, modules),
        '--rootdir',
        str(HERE),
        '--bench-config',
        args.config,
        '--stage',
        str(args.stage),
        *(['-m', f'{args.platform} and {args.delivery}'] if args.stage == Stage.HOST else []),
        '-v',
        '--tb=short',
        '-rfEs',
        '-p',
        'no:cacheprovider',
    ]
    options += ['--cells', ','.join(named)] if named else []
    options += ['--wled', args.wled] if args.wled else []
    options += ['--clip', args.clip] if args.clip else []
    options += ['--dry-run'] if args.dry_run else []
    options += ['--tv-is-free'] if with_tv else []
    options += ['--display-check'] if args.display_check else []
    # A person answers on this terminal (standby and wake, removing the app), which pytest would otherwise capture.
    options += ['-s'] if args.display_check or with_tv else []
    code = pytest.main([*options, *args.pytest], plugins=[bench])
    match code:
        case pytest.ExitCode.OK:
            return 0
        case pytest.ExitCode.USAGE_ERROR | pytest.ExitCode.NO_TESTS_COLLECTED:
            return 2
        case _:
            return 1


def restore_wled(args: Arguments, config: Config) -> int:
    saved = cast('wled_api.Snapshot', json.loads(Path(cast('str', args.snapshot)).read_text()))
    address = args.wled or config.wled
    if args.dry_run:
        print(f'Would restore WLED {address} to {saved["state"]} and maxbri {saved["cfg"]["maxbri"]}')
        return 0
    left = wled_api.restore(wled_api.Wled(address), saved)
    print('restored' if not left else f'still differs: {left}')
    return 0 if not left else 1


def _existing(name: str, config: Config, run: transport.Transport) -> installed.Existing:
    """The configured target `name` (`tv`, `linux`) as `settings restore` reaches it."""
    if name == 'tv':
        if not config.tv.target:
            raise configuration.ConfigError('[tv] target is not set')
        return tv.Tv(run, config.tv.ssh)
    if not config.linux.target.startswith(SSH):
        raise configuration.ConfigError('[linux] target is not "ssh://USER@HOST"')
    return host_deb.DebPrysm(run, config.linux.target.removeprefix(SSH))


def _restore_plan(existing: installed.Existing, backup: installed.Backup) -> list[str]:
    """What `settings restore` would do, for the dry run."""
    if isinstance(existing, host_deb.DebPrysm):
        return [
            f'GET {existing.url}/api/v1/state (refused when nothing answers)',
            f'PUT {existing.url}/api/v1/settings',
        ]
    found = cast('tv.Tv', existing)
    preferences = tv.backed_up_preferences(backup)
    return [
        *shown(commands.tv_get_state(found.target)),
        "(refused when nothing answers, or the installed package is not the backup's flavour)",
        *(shown(found.preferences_command(preferences)) if preferences is not None else []),
        'full: GET PORTAL/api/v1/state, PUT PORTAL/api/v1/settings (PORTAL as getState reports it)',
        'lighting is not switched on',
    ]


def restore_settings(args: Arguments, config: Config) -> int:
    """`settings restore TARGET BACKUP`: the backup sent to the Prysm reinstalled on TARGET, through its API."""
    name, path = cast('str', args.target), Path(cast('str', args.backup))
    try:
        backup = installed.load(path, name)
    except (OSError, ValueError) as error:
        print(f'settings restore: {error}', file=sys.stderr)
        return 2
    run = transport.Refusing() if args.dry_run else transport.Real(lambda line: None)
    existing = _existing(name, config, run)
    if args.dry_run:
        print(f'settings restore {name} {path} (dry run: nothing is executed, nothing is contacted)')
        for line in _restore_plan(existing, backup):
            print(f'  {line}')
        return 0
    try:
        done = existing.restore(backup)
    except (RuntimeError, OSError, Failed, commands.Refused) as error:
        print(f'settings restore: {error}; nothing more was sent', file=sys.stderr)
        return 1
    for line in done:
        print(f'restored: {line}')
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    args = arguments(sys.argv[1:] if argv is None else argv)
    if args.stage in (Stage.HOST, Stage.TV, Stage.ALL):
        return cells(args)
    stage = {Stage.EYES: eyes, Stage.RESTORE_WLED: restore_wled, Stage.SETTINGS: restore_settings}[args.stage]
    try:
        return stage(args, load_config(args))
    except configuration.ConfigError as error:
        print(f'config: {error}', file=sys.stderr)
        return 2


def eyes(args: Arguments, config: Config) -> int:
    """Print the plan (dry run), or ask the questions and write the report; the exit code."""
    steps = stage_eyes.steps(config)
    if args.dry_run:
        plan.print_plan(args.stage, steps, stage_eyes.CLEANUPS)
        plan.selected(steps, args.only, args.start)
        return 0
    out_dir = plan.output_directory(commands.ROOT, args.stage)
    with (out_dir / 'commands.log').open('a') as log_file:

        def log(line: str) -> None:
            print(line, flush=True)
            log_file.write(line + '\n')
            log_file.flush()

        ctx = plan.Context[EyesData](config, transport.Real(log), out_dir, log=log)
        log(f'Run directory: {out_dir}')
        outcomes = plan.execute(args.stage, steps, ctx, args.only, args.start)
        facts = {'Stage': args.stage, 'Options': ' '.join(sys.argv[1:]) or '(none)', **ctx.facts}
        passed = plan.write_report(out_dir, args.stage, outcomes, facts)
        log(f'\nReport: {out_dir / "report.md"}')
    return 0 if passed else 1


if __name__ == '__main__':
    sys.exit(main())
