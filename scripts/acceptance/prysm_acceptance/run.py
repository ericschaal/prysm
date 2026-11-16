"""Prysm hardware acceptance runner (docs/developers/acceptance.md).

  acceptance desktop [--source file|file:PATH|device] [--display-check]
  acceptance tv --tv-is-free [--flavour full|slim] [--receiver URL] [--wss-url URL] [--sweep]
  acceptance eyes [--portal URL]
  acceptance restore-wled SNAPSHOT.json

Every stage takes --config FILE (default scripts/acceptance/config.toml),
--dry-run (print every step and command; execute and contact nothing),
--only NAME[,NAME] and --from NAME (fnmatch patterns over step names; setup
and cleanup always run). Reports go to work/acceptance/<time>-<stage>/.
Exit code: 0 all passed (skips allowed), 1 a failure, 2 bad usage.
"""

import argparse
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from enum import StrEnum
import json
from pathlib import Path
import subprocess
import sys
from typing import Generic, assert_never, cast

from prysm_acceptance import commands, plan, stage_desktop, stage_eyes, stage_tv, transport, wled_api
from prysm_acceptance import config as configuration
from prysm_acceptance.commands import Command, Flavour
from prysm_acceptance.config import Config
from prysm_acceptance.options import Source
from prysm_acceptance.stage_data import DesktopData, EyesData, TvData

# The runner's project: scripts/acceptance, where config.toml lives.
HERE = Path(__file__).resolve().parents[1]


class Stage(StrEnum):
    DESKTOP = 'desktop'
    TV = 'tv'
    EYES = 'eyes'
    RESTORE_WLED = 'restore-wled'


@dataclass(frozen=True)
class Arguments:
    """The parsed command line."""

    stage: Stage
    snapshot: str | None
    config: str
    dry_run: bool
    only: list[str]
    start: str | None
    wled: str | None
    source: Source
    source_path: str | None
    display_check: bool
    no_build: bool
    seconds: int | None
    tv_is_free: bool
    flavour: list[Flavour] | None
    receiver: str | None
    wss_url: str | None
    sweep: bool
    portal: str | None


def arguments(argv: Sequence[str]) -> Arguments:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('stage', choices=[stage.value for stage in Stage])
    parser.add_argument('snapshot', nargs='?', help='restore-wled: the wled-snapshot.json of a run')
    parser.add_argument('--config', default=str(HERE / 'config.toml'))
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--only', action='append', default=[])
    parser.add_argument('--from', dest='start')
    parser.add_argument('--wled', help='WLED host[:port], overrides the config')
    parser.add_argument('--source', default='file', help='desktop: file, file:PATH or device')
    parser.add_argument('--display-check', action='store_true')
    parser.add_argument('--no-build', action='store_true', help='use target/release/prysm-headless as it is')
    parser.add_argument('--seconds', type=int, help='desktop: fps sample length')
    parser.add_argument('--tv-is-free', action='store_true', help='tv: the owner said the TV may be used now')
    parser.add_argument('--flavour', choices=[flavour.value for flavour in Flavour], action='append')
    parser.add_argument('--receiver', help='tv slim: an existing Prysm URL instead of a local prysm-headless')
    parser.add_argument('--wss-url', help='tv slim: also stream to this https:// Prysm')
    parser.add_argument('--sweep', action='store_true', help='tv: sizes x rates table (CSV)')
    parser.add_argument('--portal', help='eyes: the Prysm portal to set states on')
    values = vars(parser.parse_args(argv))
    source, source_path = cast('str', values.pop('source')), None
    if source.startswith('file:'):
        source, source_path = 'file', source[5:]
    if source not in tuple(Source):
        parser.error('--source is file, file:PATH or device')
    stage = Stage(values.pop('stage'))
    if stage == Stage.RESTORE_WLED and not values['snapshot']:
        parser.error('restore-wled needs the snapshot file')
    flavours = values.pop('flavour')
    return Arguments(
        stage=stage,
        source=Source(source),
        source_path=source_path,
        flavour=[Flavour(f) for f in flavours] if flavours else None,
        **values,
    )


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
    if args.seconds:
        config.desktop.seconds = args.seconds
    if args.flavour:
        config.tv.flavours = args.flavour
    if args.receiver:
        config.receiver.url = args.receiver
    if args.wss_url:
        config.receiver.wss_url = args.wss_url
    if args.portal:
        config.eyes_portal = args.portal
    return config


def restore_wled(args: Arguments, config: Config) -> int:
    saved = cast('wled_api.Snapshot', json.loads(Path(cast('str', args.snapshot)).read_text()))
    address = args.wled or config.wled
    if args.dry_run:
        print(f'Would restore WLED {address} to {saved["state"]} and maxbri {saved["cfg"]["maxbri"]}')
        return 0
    left = wled_api.restore(wled_api.Wled(address), saved)
    print('restored' if not left else f'still differs: {left}')
    return 0 if not left else 1


def commit() -> str:
    try:
        out = subprocess.run(
            ['git', '-C', str(commands.ROOT), 'rev-parse', '--short', 'HEAD'],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        ).stdout.strip()
        dirty = subprocess.run(['git', '-C', str(commands.ROOT), 'diff', '--quiet'], timeout=10, check=False).returncode
        return out + (' (modified tree)' if dirty else '')
    except (OSError, subprocess.SubprocessError):
        return 'unknown'


@dataclass
class StagePlan(Generic[plan.Data]):
    """One stage's steps, what its end undoes (for the dry run) and why it would refuse to start."""

    steps: list[plan.Step[plan.Data]]
    cleanups: Sequence[str] = ()
    cleanup_commands: Sequence[Command] = ()
    refusal: Callable[[], str | None] = field(default=lambda: None)


def main(argv: Sequence[str] | None = None) -> int:
    args = arguments(sys.argv[1:] if argv is None else argv)
    try:
        config = load_config(args)
    except configuration.ConfigError as error:
        print(f'config: {error}', file=sys.stderr)
        return 2
    match args.stage:
        case Stage.DESKTOP:
            return run(
                args,
                StagePlan(stage_desktop.steps(config, args), stage_desktop.cleanups(config, args)),
                plan.Context[DesktopData],
                config,
            )
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
        facts = {'Commit': commit(), 'Stage': args.stage, 'Options': ' '.join(sys.argv[1:]) or '(none)', **ctx.facts}
        passed = plan.write_report(out_dir, args.stage, outcomes, facts)
        log(f'\nReport: {out_dir / "report.md"}')
    return 0 if passed else 1


if __name__ == '__main__':
    sys.exit(main())
