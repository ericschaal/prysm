"""The acceptance runner's colour check, fake WLED, configuration, artifacts, step runner and dry runs."""

from collections.abc import Callable, Sequence
import contextlib
import datetime
import hashlib
import io
import json
from pathlib import Path
import socket
import tempfile
import threading
import time
from typing import Any, Never, cast
import unittest

from acceptance_fakes import edges_event, untyped

from prysm_acceptance import (
    artifact,
    colour_page,
    config,
    edges_clip,
    host_cells,
    plan,
    prysm_api,
    run,
    stage_eyes,
    transport,
    web,
    wled_api,
)
from prysm_acceptance.checks import Answer, Bench, Failed
from prysm_acceptance.fake_wled import FakeWled
from prysm_acceptance.host import Host
from prysm_acceptance.plan import Mode
from prysm_acceptance.prysm_api import ColorsEvent, Edge
from prysm_acceptance.transport import Result

# The runner's project: scripts/acceptance.
ACCEPTANCE = Path(__file__).resolve().parents[1]

EXAMPLE = ACCEPTANCE / 'config.example.toml'

# Each edge's two colours, as `edges_clip.expected` gives them.
Pairs = dict[Edge, tuple[str, ...]]


def step(number: int, change: Callable[[Pairs], None] | None = None) -> ColorsEvent:
    """The clip's colours at step `number`, as the stream sends them; `change(pairs)` alters the edges."""
    pairs = edges_clip.expected(number % edges_clip.STEPS)
    if change:
        change(pairs)
    return edges_event(pairs)


def timeline(
    first: int = 0, changes: Sequence[float] = (1.3, 3.3, 5.3, 7.3, 9.3), change: Callable[[Pairs], None] | None = None
) -> list[tuple[float, ColorsEvent]]:
    """Timed samples over 10 s: step `first` at once, the next one at each change."""
    return [(0.1, step(first, change))] + [(at, step(first + i, change)) for i, at in enumerate(changes, 1)]


class TheColourCheck(unittest.TestCase):
    """The stepping clip's check on the colours stream: direction, steps and pace."""

    def judged(self, samples: Sequence[tuple[float, ColorsEvent]]) -> tuple[str, dict[str, object]]:
        return edges_clip.judge(samples, 10.0)

    def fails(self, samples: Sequence[tuple[float, ColorsEvent]], message: str) -> Failed:
        with self.assertRaisesRegex(Failed, message) as failed:
            self.judged(samples)
        return failed.exception

    def test_the_stream_reports_each_edge_from_one_corner_to_the_next(self) -> None:
        # Top and bottom left to right, left and right top to bottom; corners red, green, blue, yellow
        # from the top-left.
        self.assertEqual(
            edges_clip.expected(0),
            {
                'top': ('red', 'green'),
                'right': ('green', 'blue'),
                'bottom': ('yellow', 'blue'),
                'left': ('red', 'yellow'),
            },
        )
        # One step on, every colour has moved one corner clockwise.
        self.assertEqual(
            edges_clip.expected(1),
            {
                'top': ('yellow', 'red'),
                'right': ('red', 'green'),
                'bottom': ('blue', 'green'),
                'left': ('yellow', 'blue'),
            },
        )
        self.assertEqual(edges_clip.corner_hex(), ('ff0000', '00ff00', '0000ff', 'ffff00'))

    def test_the_clip_passes(self) -> None:
        detail, evidence = self.judged(timeline(first=3))
        self.assertIn('6 steps in order', detail)
        self.assertEqual([number for _, number in untyped(evidence)['steps']], [3, 0, 1, 2, 3, 0])
        self.assertEqual(evidence['intervals'], [2.0, 2.0, 2.0, 2.0])

    def test_the_overlay_over_the_bottom_is_judged_against_the_clip(self) -> None:
        def overlaid(pairs: Pairs) -> None:
            # The overlay over the bottom edge, reaching into the sides' lower halves, which are not read.
            pairs['bottom'] = ('white', 'white')
            pairs['left'] = (pairs['left'][0], 'white')
            pairs['right'] = (pairs['right'][0], 'gray')

        detail, _ = edges_clip.judge(timeline(change=overlaid), 10.0, edges_clip.decode_overlaid)
        self.assertIn('6 steps in order', detail)
        self.assertIn('the bottom edge the overlay (white)', detail)
        self.assertEqual(
            edges_clip.decode_overlaid(step(0))[1], 'no overlay over the bottom edge: it shows yellow, blue at step 0'
        )

        def hidden_top(pairs: Pairs) -> None:
            pairs['top'] = pairs['bottom'] = ('white', 'white')

        hidden = step(0, hidden_top)
        self.assertIn('wrong colour outside the overlay', edges_clip.decode_overlaid(hidden)[1] or '')
        with self.assertRaisesRegex(Failed, 'no overlay over the bottom edge'):
            edges_clip.judge(timeline(), 10.0, edges_clip.decode_overlaid)

    def test_only_the_quarters_between_corner_and_middle_are_read(self) -> None:
        event = step(0)
        event['top'][0] = event['top'][3] = event['top'][4] = event['top'][7] = [70, 70, 70]
        self.assertEqual(edges_clip.decode(event), (0, None))
        event['top'][2] = [70, 70, 70]
        self.assertEqual(edges_clip.decode(event)[1], 'wrong colour: top shows mixed, green, step 0 has red, green')

    def test_a_wrong_colour_fails(self) -> None:
        def gray(pairs: Pairs) -> None:
            pairs['right'] = ('gray', pairs['right'][1])

        failed = self.fails(timeline(change=gray), r'^wrong colour: right shows gray, ')
        self.assertTrue(untyped(failed.evidence)['samples'][0]['edges']['right'].startswith('gray'))

    def test_a_reversed_edge_fails(self) -> None:
        def mirror(pairs: Pairs) -> None:
            pairs['left'] = pairs['left'][::-1]

        self.fails(timeline(change=mirror), r'^reversed edge: left shows yellow, red, step 0 has red, yellow')

    def test_swapped_edges_fail(self) -> None:
        def swap(pairs: Pairs) -> None:
            pairs['top'], pairs['right'] = pairs['right'], pairs['top']

        self.fails(timeline(change=swap), r"^swapped edges: top shows green, blue, right's colours")

    def test_a_frozen_picture_fails(self) -> None:
        failed = self.fails([(0.1, step(2))], r'^frozen picture: step 2 for the whole 10 s')
        self.assertEqual(untyped(failed.evidence)['steps'], [[0.1, 2]])
        self.fails(timeline(changes=(1.3,)), r'^wrong pace: no change for 8.70 s after the last change')

    def test_steps_at_the_wrong_pace_fail(self) -> None:
        failed = self.fails(
            timeline(changes=(1.3, 3.9, 6.5, 9.1)), r'^wrong pace: steps changed \[2.6, 2.6, 2.6\] s apart'
        )
        self.assertEqual(untyped(failed.evidence)['intervals'], [2.6, 2.6, 2.6])
        self.fails(timeline(changes=(1.3, 2.3, 3.3, 4.3, 5.3, 6.3, 7.3, 8.3, 9.3)), r'^wrong pace')

    def test_a_skipped_step_fails(self) -> None:
        samples = timeline()
        samples[2] = (3.3, step(3))
        self.fails(samples, r'^steps out of order: step 1 at 1.3 s, then step 3')

    def test_a_sample_on_a_step_boundary_is_a_short_transition(self) -> None:
        # Caught mid-change: top and right already show step 2, bottom and left still step 1.
        mixed = step(1)
        untyped(mixed).update({edge: step(2)[edge] for edge in ('top', 'right')})
        samples = timeline()
        samples[2:3] = [(3.3, mixed), (3.5, step(2))]
        detail, evidence = self.judged(samples)
        self.assertIn('in order', detail)
        self.assertEqual(evidence['intervals'], [2.2, 1.8, 2.0, 2.0])
        self.assertTrue(untyped(evidence)['samples'][2]['fault'].startswith('swapped edges'))

    def test_a_short_fault_within_a_step_fails(self) -> None:
        mirrored = step(1)
        mirrored['left'] = mirrored['left'][::-1]
        samples = timeline()
        samples[2:2] = [(2.0, mirrored), (2.2, step(1))]
        failed = self.fails(samples, r'^reversed edge: left shows blue, yellow, step 1 has yellow, blue \(for 0.2 s')
        self.assertEqual(untyped(failed.evidence)['steps'], [[0.1, 0], [1.3, 1]])

    def test_too_long_a_transition_fails(self) -> None:
        mixed = step(1)
        untyped(mixed).update({edge: step(2)[edge] for edge in ('top', 'right')})
        samples = timeline()
        samples[2:3] = [(3.3, mixed), (3.7, step(2))]
        self.fails(samples, r'^transition too long: from step 1 to step 2 took 0.4 s \(at most 0.3 s\)')


class FakeWledBehaviour(unittest.TestCase):
    def test_ddp_makes_it_live_and_restore_puts_it_back(self) -> None:
        device = FakeWled(http=('127.0.0.1', 0), ddp=('127.0.0.1', 0)).start()
        self.addCleanup(device.close)
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sender:
            sender.sendto(bytes([0x41, 0, 1, 1, 0, 0, 0, 0, 0, 3, 255, 0, 0]), untyped(device.udp).getsockname())
        time.sleep(0.3)
        wled = wled_api.Wled(device.http_address)
        info = wled.si()['info']
        self.assertEqual((info['live'], info['lm'], info['lip']), (True, 'DDP', '127.0.0.1'))
        saved = wled_api.snapshot(wled)
        wled.post_state({'on': False, 'lor': 2, 'bri': 3})
        wled.post_cfg({'if': {'live': {'maxbri': True}}})
        wled.post_state({'transition': 0})
        self.assertEqual(set(wled_api.differences(wled, saved)), {'on', 'bri', 'lor', 'transition', 'maxbri'})
        self.assertEqual(wled_api.restore(wled, saved), {})


class Sender(unittest.TestCase):
    """WLED must report the sender the host seam expects, not any live DDP."""

    def test_another_sender_fails(self) -> None:
        device = FakeWled(http=('127.0.0.1', 0), ddp=('127.0.0.1', 0)).start()
        self.addCleanup(device.close)
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sender:
            sender.sendto(bytes([0x41, 0, 1, 1, 0, 0, 0, 0, 0, 3, 255, 0, 0]), untyped(device.udp).getsockname())
        time.sleep(0.3)

        class Seam:
            def sender(self) -> str:
                return '192.0.2.10'

        prysm = prysm_api.Prysm('http://127.0.0.1:9')  # not reached: the sender fails first
        with self.assertRaisesRegex(Failed, r'another sender: 127.0.0.1 \(expected 192.0.2.10\)'):
            host_cells.live_from_host(prysm, wled_api.Wled(device.http_address), cast('Host', Seam()))


class ColourPage(unittest.TestCase):
    def test_the_page_reports_what_it_shows(self) -> None:
        page = colour_page.ColourPage(0).start()
        self.addCleanup(page.close)
        self.assertIn(b'Prysm colour fields', web.request('GET', page.url).body)
        name, edges = colour_page.FIELDS[0]
        # Stand in for the browser: read the field, then report it shown.
        shown: list[str] = []

        def browser() -> None:
            time.sleep(0.2)
            field = untyped(web.get_json(page.url + 'field'))
            shown.append(field['name'])
            web.request('POST', page.url + 'shown', {'name': field['name'], 'full': True})

        threading.Thread(target=browser).start()
        self.assertTrue(page.show(name, edges, seconds=5))
        self.assertEqual(shown, ['red'])

    def test_edges_are_named_by_their_nearest_colour(self) -> None:
        event: ColorsEvent = {
            'top': [[90, 90, 90]] * 2 + [[230, 60, 50]] * 4 + [[90, 90, 90]] * 2,
            'right': [[40, 230, 40]] * 8,
            'bottom': [[60, 60, 220]] * 8,
            'left': [],
        }
        ok, found = colour_page.matches(event, colour_page.FIELDS[4][1])
        self.assertFalse(ok)  # the left edge has no colours
        self.assertEqual(untyped(found['top'])['seen'], 'red')
        self.assertIsNone(found['left'])


class Configuration(unittest.TestCase):
    def test_the_example_parses(self) -> None:
        values = config.load(EXAMPLE)
        self.assertEqual((values.tv.ssh, values.tv.installed), ('root@192.168.1.50', config.Installed.REFUSE))
        self.assertEqual(values.linux.installed, config.Installed.REFUSE)
        self.assertEqual((values.mac.target, values.mac.device), ('local', '0x1100000345f2131'))
        self.assertEqual(untyped(values.mac.capture).fps, 30)
        self.assertTrue(values.linux.target.startswith('ssh://'))

    def test_mistakes_are_refused(self) -> None:
        cases: tuple[dict[str, object], ...] = (
            {'checks': {'sekonds': 3}},
            {'desktop': {'seconds': 3}},
            {'wled': {'address': 'http://wled'}},
            {'tv': {'target': 'root@tv'}},
            {'tv': {'target': 'ssh://-oProxyCommand=sh'}},
            {'tv': {'mac': '00:00:5E:00:53'}},
            {'tv': {'installed': 'keep'}},
            {'tv': {'address': 'tv.example'}},
            {'tv': {'restore_flavour': 'none'}},
            {'linux': {'installed': 'reinstall'}},
            {'mac': {'installed': 'replace'}},
            {'checks': {'seconds': '3'}},
            {'checks': {'fps_tolerance': True}},
            {'checks': {'fps_tolerance': 2}},
            {'sweep': {'sizes': ['640x360']}},
            {'receiver': {'url': 'http://192.0.2.20:8788'}},
            {'leds': {'top': -1}},
            {'tv': {'ssh': 'root@tv'}},
            {'mac': {'target': 'pi@host'}},
            {'mac': {'deb': 'dist/prysm-headless-linux-arm64.deb'}},
            {'mac': {'capture': {'width': 0}}},
            {'mac': {'listen': '127.0.0.1:18787 --start'}},
            {'linux': {'target': 'ssh://-oProxyCommand=sh'}},
        )
        for data in cases:
            with self.assertRaises(config.ConfigError, msg=data):
                config.parse(data)

    def test_a_remote_mac_is_not_implemented(self) -> None:
        with self.assertRaisesRegex(config.ConfigError, 'a remote Mac is not implemented'):
            config.parse({'mac': {'target': 'ssh://me@mac.example'}})

    def test_a_whole_number_is_a_float_but_true_is_not(self) -> None:
        feed = {'adb_serial': 'ABCD1234', 'settle_seconds': 3}
        self.assertEqual(untyped(config.parse({'mac': {'feed': {'chromecast': feed}}}).mac.feed).settle_seconds, 3)
        with self.assertRaisesRegex(config.ConfigError, 'expected float'):
            config.parse({'mac': {'feed': {'chromecast': {**feed, 'settle_seconds': True}}}})


class Artifacts(unittest.TestCase):
    def setUp(self) -> None:
        self.dist = Path(tempfile.mkdtemp())
        self.binary = self.dist / 'prysm-headless-macos-arm64'
        self.binary.write_bytes(b'prysm')
        self.sha = hashlib.sha256(b'prysm').hexdigest()

    def manifest(self, sha256: str) -> None:
        entry = {'version': '1.0.0-alpha.0', 'commit': 'abc123', 'dirty': False, 'sha256': sha256, 'built_at': 'x'}
        (self.dist / 'manifest.json').write_text(json.dumps({'artifacts': {self.binary.name: entry}}))

    def test_hash_and_age_without_a_manifest(self) -> None:
        found = artifact.inspect(self.binary)
        self.assertEqual((found.sha256, found.version), (self.sha, None))
        self.assertIn('version and commit unknown', found.facts()['manifest'])

    def test_a_matching_manifest_gives_the_version_and_commit(self) -> None:
        self.manifest(self.sha)
        facts = artifact.inspect(self.binary).facts()
        self.assertEqual((facts['version'], facts['commit']), ('1.0.0-alpha.0', 'abc123'))

    def test_a_manifest_of_another_build_is_ignored(self) -> None:
        self.manifest('0' * 64)
        self.assertIsNone(artifact.inspect(self.binary).entry)

    def test_older_than_the_last_commit_warns(self) -> None:
        found = artifact.inspect(self.binary)
        later = (found.modified + datetime.timedelta(hours=1)).isoformat()
        earlier = (found.modified - datetime.timedelta(hours=1)).isoformat()
        warning = artifact.older_than_checkout(
            found, transport.Transcript([('local.git-last-commit-time', Result(0, later))])
        )
        self.assertIn('older than the last commit', str(warning))
        record = transport.Transcript([('local.git-last-commit-time', Result(0, earlier))])
        self.assertIsNone(artifact.older_than_checkout(found, record))
        no_git = transport.Transcript([('local.git-last-commit-time', Result(128, '', 'not a repository'))])
        self.assertIsNone(artifact.older_than_checkout(found, no_git))


class TheBench(unittest.TestCase):
    def test_every_undo_runs_last_first_and_failures_are_reported(self) -> None:
        bench = Bench(config.parse({}), transport.Refusing(), Path(tempfile.mkdtemp()))
        done: list[str] = []

        def broken() -> Never:
            raise RuntimeError('unplugged')

        bench.cleanup('a', 'A', lambda: done.append('a'))
        bench.cleanup('b', 'B', broken)
        bench.cleanup('c', 'C', lambda: done.append('c'))
        self.assertEqual(bench.undo(), ['b: RuntimeError: unplugged'])
        self.assertEqual((done, bench.cleanups), (['c', 'a'], []))


class Plan(unittest.TestCase):
    def context(self) -> plan.Context[Any]:
        return plan.Context[Any](
            config.parse({}), transport.Refusing(), Path(tempfile.mkdtemp()), log=lambda line: None
        )

    def test_selection_keeps_setup_steps(self) -> None:
        steps: list[plan.Step[Any]] = [
            plan.Step('a.setup', 'S', 'h', Mode.SETUP, lambda ctx: None),
            plan.Step('a.one', 'One', 'h', Mode.AUTO, lambda ctx: None),
            plan.Step('a.two', 'Two', 'h', Mode.AUTO, lambda ctx: None),
            plan.Step('a.three', 'Three', 'h', Mode.AUTO, lambda ctx: None),
        ]
        self.assertEqual(plan.selected(steps, ['a.t*'], None), {'a.two', 'a.three'})
        self.assertEqual(plan.selected(steps, [], 'a.two'), {'a.two', 'a.three'})
        names = [o.name for o in plan.execute('a', steps, self.context(), only=['a.three'])]
        self.assertEqual(names, ['a.setup', 'a.three'])

    def test_a_failed_setup_blocks_but_cleanups_run_and_the_report_says_so(self) -> None:
        ctx = self.context()
        restored: list[bool] = []

        def setup(ctx: Bench) -> Never:
            ctx.cleanup('a.restore', 'Restore', lambda: restored.append(True) or 'restored')
            raise Failed('no device', {'why': 'unplugged'})

        steps: list[plan.Step[Any]] = [
            plan.Step('a.setup', 'S', 'h', Mode.SETUP, setup),
            plan.Step('a.check', 'C', 'h', Mode.AUTO, lambda c: 1),
            plan.Step('a.gamma', 'G', 'not observable', Mode.SKIP),
        ]
        outcomes = plan.execute('a', steps, ctx)
        self.assertEqual(
            [(o.name, o.status) for o in outcomes],
            [
                ('a.setup', plan.Status.FAIL),
                ('a.check', plan.Status.SKIP),
                ('a.gamma', plan.Status.SKIP),
                ('a.restore', plan.Status.PASS),
            ],
        )
        self.assertEqual(restored, [True])
        self.assertFalse(plan.write_report(ctx.out_dir, 'a', outcomes, {'Commit': 'abc'}))
        report = json.loads((ctx.out_dir / 'report.json').read_text())
        self.assertFalse(report['passed'])
        self.assertIn('| `a.setup` | setup | FAIL | no device |', (ctx.out_dir / 'report.md').read_text())

    def test_ctrl_c_runs_the_cleanups(self) -> None:
        ctx = self.context()
        restored: list[bool] = []

        def interrupted(ctx: Bench) -> Never:
            ctx.cleanup('a.restore', 'Restore', lambda: restored.append(True))
            raise KeyboardInterrupt

        step = plan.Step[Any]('a.check', 'C', 'h', Mode.AUTO, interrupted)
        outcomes = plan.execute('a', [step], ctx)
        self.assertEqual(restored, [True])
        self.assertIn('interrupted', [o.name for o in outcomes])

    def test_guided_answers_decide_the_result(self) -> None:
        ctx = self.context()
        answers = iter([(Answer.YES, ''), (Answer.NO, 'flickers'), (Answer.SKIP, '')])

        def ask(question: str, choices: tuple[Answer, ...] = ()) -> tuple[Answer, str]:
            return next(answers)

        ctx.ask = ask
        steps: list[plan.Step[Any]] = [
            plan.Step(f'e.{i}', 'Q', 'h', Mode.GUIDED, stage_eyes.question('Q?')) for i in range(3)
        ]
        self.assertEqual(
            [o.status for o in plan.execute('eyes', steps, ctx)], [plan.Status.PASS, plan.Status.FAIL, plan.Status.SKIP]
        )


class DryRun(unittest.TestCase):
    def dry(self, *arguments: str) -> tuple[int, str]:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = run.main([*arguments, '--dry-run', '--config', str(EXAMPLE)])
        return code, output.getvalue()

    def test_eyes_prints_its_plan(self) -> None:
        code, text = self.dry('eyes')
        self.assertEqual(code, 0)
        self.assertIn('eyes.cold-boot', text)
        self.assertIn('nothing is executed', text)

    def test_eyes_keeps_only_what_a_person_must_judge(self) -> None:
        _, text = self.dry('eyes')
        self.assertIn('eyes.flicker-no-rerender', text)
        for gone in ('start-with-tv', 'diagnostics', 'remove-dialog', 'flicker-cold-start', 'flicker-retry-bar'):
            self.assertNotIn(gone, text)


if __name__ == '__main__':
    unittest.main()
