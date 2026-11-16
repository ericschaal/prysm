#!/usr/bin/env python3
"""Prysm hardware acceptance runner (docs/developers/acceptance.md).

  run.py desktop [--source file|file:PATH|device] [--display-check]
  run.py tv --tv-is-free [--flavour full|slim] [--receiver URL] [--wss-url URL] [--sweep]
  run.py eyes [--portal URL]
  run.py restore-wled SNAPSHOT.json

Every stage takes --config FILE (default scripts/acceptance/config.toml),
--dry-run (print every step and command; execute and contact nothing),
--only NAME[,NAME] and --from NAME (fnmatch patterns over step names; setup
and cleanup always run). Reports go to work/acceptance/<time>-<stage>/.
Exit code: 0 all passed (skips allowed), 1 a failure, 2 bad usage.
"""
import argparse
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import commands  # noqa: E402
import config as configuration  # noqa: E402
import plan  # noqa: E402
import stage_desktop  # noqa: E402
import stage_eyes  # noqa: E402
import stage_tv  # noqa: E402
import transport  # noqa: E402
import wled_api  # noqa: E402

STAGES = {'desktop': stage_desktop, 'tv': stage_tv, 'eyes': stage_eyes}


def arguments(argv):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('stage', choices=[*STAGES, 'restore-wled'])
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
    parser.add_argument('--flavour', choices=commands.FLAVOURS, action='append')
    parser.add_argument('--receiver', help='tv slim: an existing Prysm URL instead of a local prysm-headless')
    parser.add_argument('--wss-url', help='tv slim: also stream to this https:// Prysm')
    parser.add_argument('--sweep', action='store_true', help='tv: sizes x rates table (CSV)')
    parser.add_argument('--portal', help='eyes: the Prysm portal to set states on')
    args = parser.parse_args(argv)
    args.source_path = None
    if args.source.startswith('file:'):
        args.source, args.source_path = 'file', args.source[5:]
    if args.source not in ('file', 'device'):
        parser.error('--source is file, file:PATH or device')
    if args.stage == 'restore-wled' and not args.snapshot:
        parser.error('restore-wled needs the snapshot file')
    return args


def load_config(args):
    path = Path(args.config)
    if path.exists():
        config = configuration.load(path)
    elif args.dry_run:
        config = configuration.load(HERE / 'config.example.toml')
    else:
        raise configuration.ConfigError(f'{path} does not exist: copy config.example.toml and fill it in')
    if args.wled:
        config.wled = configuration._address(args.wled, '--wled')
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


def restore_wled(args, config):
    import json
    saved = json.loads(Path(args.snapshot).read_text())
    address = args.wled or config.wled
    if args.dry_run:
        print(f'Would restore WLED {address} to {saved["state"]} and maxbri {saved["cfg"]["maxbri"]}')
        return 0
    left = wled_api.restore(wled_api.Wled(address), saved)
    print('restored' if not left else f'still differs: {left}')
    return 0 if not left else 1


def commit():
    try:
        out = subprocess.run(['git', '-C', str(commands.ROOT), 'rev-parse', '--short', 'HEAD'],
                             capture_output=True, text=True, timeout=10).stdout.strip()
        dirty = subprocess.run(['git', '-C', str(commands.ROOT), 'diff', '--quiet'], timeout=10).returncode
        return out + (' (modified tree)' if dirty else '')
    except (OSError, subprocess.SubprocessError):
        return 'unknown'


def main(argv=None):
    args = arguments(sys.argv[1:] if argv is None else argv)
    try:
        config = load_config(args)
    except configuration.ConfigError as error:
        print(f'config: {error}', file=sys.stderr)
        return 2
    if args.stage == 'restore-wled':
        return restore_wled(args, config)
    stage = STAGES[args.stage]
    steps = stage.steps(config, args)
    if args.dry_run:
        cleanups = stage.cleanups(config, args) if hasattr(stage, 'cleanups') else getattr(stage, 'CLEANUPS', ())
        plan.print_plan(args.stage, steps, cleanups,
                        getattr(stage, 'cleanup_commands', lambda config: ())(config))
        plan.selected(steps, args.only, args.start)
        return 0
    refusal = getattr(stage, 'refuse', lambda config, args: None)(config, args)
    if refusal:
        print(refusal, file=sys.stderr)
        return 2
    out_dir = plan.output_directory(commands.ROOT, args.stage)
    log_file = open(out_dir / 'commands.log', 'a')

    def log(line):
        print(line, flush=True)
        log_file.write(line + '\n')
        log_file.flush()
    ctx = plan.Context(config, args, transport.Real(log), out_dir, log=log)
    log(f'Run directory: {out_dir}')
    outcomes = plan.execute(args.stage, steps, ctx, args.only, args.start)
    facts = {'Commit': commit(), 'Stage': args.stage, 'Options': ' '.join(sys.argv[1:]) or '(none)',
             **ctx.data.get('facts', {})}
    passed = plan.write_report(out_dir, args.stage, outcomes, facts)
    log(f'\nReport: {out_dir / "report.md"}')
    log_file.close()
    return 0 if passed else 1


if __name__ == '__main__':
    sys.exit(main())
