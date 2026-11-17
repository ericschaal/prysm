"""Prysm hardware acceptance runner (docs/developers/acceptance.md).

  acceptance host mac|linux [--delivery binary] [--source file|file:PATH|device] [--display-check] [PYTEST ARGS]
  acceptance tv --tv-is-free [--flavour full|slim] [--receiver URL] [--wss-url URL] [--sweep]
  acceptance eyes [--portal URL]
  acceptance restore-wled SNAPSHOT.json

Each takes --config FILE (default scripts/acceptance/config.toml) and
--dry-run (print what would run; execute and contact nothing). `host` runs
the host cells with pytest: other arguments go to pytest (-k, -m, -x, -v ...);
naming the source names the cell, which then fails instead of skipping when
its artifact or tool is missing. `tv` and `eyes` take --only NAME[,NAME] and
--from NAME (patterns over step names; setup and cleanup always run).
Reports go to work/acceptance/<time>-<stage>/.
Exit code: 0 all passed (skips allowed), 1 a failure, 2 bad usage.
"""

import argparse
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from enum import StrEnum
import json
from pathlib import Path
import sys
from typing import Generic, assert_never, cast

import pytest

from prysm_acceptance import bench, commands, plan, stage_eyes, stage_tv, transport, wled_api
from prysm_acceptance import config as configuration
from prysm_acceptance.commands import Command, Flavour
from prysm_acceptance.config import Config
from prysm_acceptance.options import Cell, Delivery, Platform, Source
from prysm_acceptance.stage_data import EyesData, TvData

# The runner's project: scripts/acceptance, where config.toml lives.
HERE = Path(__file__).resolve().parents[1]
CELLS = Path(__file__).with_name('host_cells.py')


class Stage(StrEnum):
    HOST = 'host'
    TV = 'tv'
    EYES = 'eyes'
    RESTORE_WLED = 'restore-wled'


@dataclass(frozen=True)
class Arguments:
    """The parsed command line; each stage reads its own."""

    stage: Stage
    config: str
    dry_run: bool
    wled: str | None = None
    # host
    platform: Platform = Platform.MAC
    delivery: Delivery = Delivery.BINARY
    source: Source | None = None
    clip: str | None = None
    display_check: bool = False
    pytest: list[str] = field(default_factory=list[str])
    # tv and eyes
    only: list[str] = field(default_factory=list[str])
    start: str | None = None
    tv_is_free: bool = False
    flavour: list[Flavour] | None = None
    receiver: str | None = None
    wss_url: str | None = None
    sweep: bool = False
    portal: str | None = None
    # restore-wled
    snapshot: str | None = None


def arguments(argv: Sequence[str]) -> Arguments:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument('--config', default=str(HERE / 'config.toml'))
    common.add_argument('--dry-run', action='store_true')
    common.add_argument('--wled', help='WLED host[:port], overrides the config')
    stages = parser.add_subparsers(dest='stage', required=True)
    host = stages.add_parser(Stage.HOST, parents=[common], help='the host cells (pytest)')
    host.add_argument('platform', choices=[platform.value for platform in Platform], help='where Prysm runs')
    host.add_argument('--delivery', choices=[delivery.value for delivery in Delivery], default=Delivery.BINARY)
    host.add_argument('--source', help='file, file:PATH or device (default: every source of the platform)')
    host.add_argument('--display-check', action='store_true', help="device: colour fields on this computer's display")
    selection = argparse.ArgumentParser(add_help=False)
    selection.add_argument('--only', action='append', default=[])
    selection.add_argument('--from', dest='start')
    tv = stages.add_parser(Stage.TV, parents=[common, selection], help='both TV packages')
    tv.add_argument('--tv-is-free', action='store_true', help='the owner said the TV may be used now')
    tv.add_argument('--flavour', choices=[flavour.value for flavour in Flavour], action='append')
    tv.add_argument('--receiver', help='slim: an existing Prysm URL instead of a local prysm-headless')
    tv.add_argument('--wss-url', help='slim: also stream to this https:// Prysm')
    tv.add_argument('--sweep', action='store_true', help='sizes x rates table (CSV)')
    eyes = stages.add_parser(Stage.EYES, parents=[common, selection], help='what a person judges')
    eyes.add_argument('--portal', help='the Prysm portal to set states on')
    restore = stages.add_parser(Stage.RESTORE_WLED, parents=[common], help='put WLED back from a snapshot')
    restore.add_argument('snapshot', help='the wled-snapshot.json of a run')
    values, rest = parser.parse_known_args(argv)
    found = vars(values)
    stage = Stage(found.pop('stage'))
    if rest and stage != Stage.HOST:
        parser.error(f'unrecognized arguments: {" ".join(rest)}')
    if stage == Stage.HOST:
        source = cast('str | None', found.pop('source'))
        if source and source.startswith('file:'):
            found['clip'], source = source[5:], Source.FILE
        if source and source not in tuple(Source):
            parser.error('--source is file, file:PATH or device')
        found |= {
            'source': Source(source) if source else None,
            'platform': Platform(found['platform']),
            'delivery': Delivery(found['delivery']),
            'pytest': rest,
        }
    if found.get('flavour'):
        found['flavour'] = [Flavour(f) for f in found['flavour']]
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
    if args.flavour:
        config.tv.flavours = args.flavour
    if args.receiver:
        config.receiver.url = args.receiver
    if args.wss_url:
        config.receiver.wss_url = args.wss_url
    if args.portal:
        config.eyes_portal = args.portal
    return config


def host(args: Arguments) -> int:
    """The host cells under pytest, with the `bench` plugin; pytest's exit code as the runner's."""
    named = [Cell(args.platform, args.delivery, args.source).id] if args.source else []
    options = [
        str(CELLS),
        '--rootdir',
        str(HERE),
        '--bench-config',
        args.config,
        '-m',
        f'{args.platform} and {args.delivery}',
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
    # A person answers on this terminal, which pytest would otherwise capture.
    options += ['--display-check', '-s'] if args.display_check else []
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


@dataclass
class StagePlan(Generic[plan.Data]):
    """One stage's steps, what its end undoes (for the dry run) and why it would refuse to start."""

    steps: list[plan.Step[plan.Data]]
    cleanups: Sequence[str] = ()
    cleanup_commands: Sequence[Command] = ()
    refusal: Callable[[], str | None] = field(default=lambda: None)


def main(argv: Sequence[str] | None = None) -> int:
    args = arguments(sys.argv[1:] if argv is None else argv)
    if args.stage == Stage.HOST:
        return host(args)
    try:
        config = load_config(args)
    except configuration.ConfigError as error:
        print(f'config: {error}', file=sys.stderr)
        return 2
    match args.stage:
        case Stage.TV:
            return run(
                args,
                StagePlan(
                    stage_tv.steps(config, args),
                    stage_tv.CLEANUPS,
                    stage_tv.cleanup_commands(config),
                    lambda: stage_tv.refuse(config, args),
                ),
                plan.Context[TvData],
                config,
            )
        case Stage.EYES:
            return run(
                args,
                StagePlan(stage_eyes.steps(config, args), stage_eyes.CLEANUPS),
                plan.Context[EyesData],
                config,
            )
        case Stage.RESTORE_WLED:
            return restore_wled(args, config)
        case _:
            assert_never(args.stage)


def run(
    args: Arguments,
    stage: StagePlan[plan.Data],
    context: Callable[..., plan.Context[plan.Data]],
    config: Config,
) -> int:
    """Print the plan (dry run), or run the stage and write its report; the exit code."""
    if args.dry_run:
        plan.print_plan(args.stage, stage.steps, stage.cleanups, stage.cleanup_commands)
        plan.selected(stage.steps, args.only, args.start)
        return 0
    refusal = stage.refusal()
    if refusal:
        print(refusal, file=sys.stderr)
        return 2
    out_dir = plan.output_directory(commands.ROOT, args.stage)
    with (out_dir / 'commands.log').open('a') as log_file:

        def log(line: str) -> None:
            print(line, flush=True)
            log_file.write(line + '\n')
            log_file.flush()

        ctx = context(config, args, transport.Real(log), out_dir, log=log)
        log(f'Run directory: {out_dir}')
        outcomes = plan.execute(args.stage, stage.steps, ctx, args.only, args.start)
        facts = {'Stage': args.stage, 'Options': ' '.join(sys.argv[1:]) or '(none)', **ctx.facts}
        passed = plan.write_report(out_dir, args.stage, outcomes, facts)
        log(f'\nReport: {out_dir / "report.md"}')
    return 0 if passed else 1


if __name__ == '__main__':
    sys.exit(main())
