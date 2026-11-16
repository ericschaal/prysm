"""The acceptance runner's checks against a fake WLED and a fake Prysm on loopback, its plan and report."""
# pyright: reportPrivateUsage=false
# (The tests also check the desktop stage's own `_feed`.)

from collections.abc import Callable, Sequence
import contextlib
import io
import json
from pathlib import Path
import socket
import tempfile
import threading
import time
from typing import Any, ClassVar, Never
import unittest

from acceptance_fakes import NO_OPTIONS, FakePrysm, edges_event, untyped

from prysm_acceptance import (
    colour_page,
    config,
    edges_clip,
    feed,
    modes,
    plan,
    prysm_api,
    run,
    stage_desktop,
    stage_eyes,
    stream_version,
    transport,
    web,
    wled_api,
)
from prysm_acceptance.commands import Command, Flavour
from prysm_acceptance.config import Preset
from prysm_acceptance.fake_wled import FakeWled
from prysm_acceptance.options import Source
from prysm_acceptance.plan import Answer, Bench, Mode
from prysm_acceptance.prysm_api import CaptureMode, ColorsEvent, Edge, Span
from prysm_acceptance.stage_data import DesktopData

# The runner's project: scripts/acceptance.
ACCEPTANCE = Path(__file__).resolve().parents[1]

EXAMPLE = ACCEPTANCE / 'config.example.toml'

# Each edge's two colours, as `edges_clip.expected` gives them.
Pairs = dict[Edge, tuple[str, ...]]


class Devices(unittest.TestCase):
    """A fake WLED and a fake Prysm that drives it, with the desktop checks' context."""

    def setUp(self) -> None:
        self.wled = FakeWled(leds=162).start()
        self.addCleanup(self.wled.close)
        settings: dict[str, object] = {
            'output': {
                'kind': 'wled',
                'address': self.wled.http_address,
                'dither': True,
                'leds': {'top': 1, 'right': 1, 'bottom': 1, 'left': 1},
            },
            'capture': {'kind': 'file', 'path': 'clip.mp4', 'width': 640, 'height': 360, 'fps': 30},
            'processing': {},
        }
        self.prysm = FakePrysm(self.wled, settings).start()
        self.addCleanup(self.prysm.close)
        values = config.parse({'wled': {'address': self.wled.http_address}, 'desktop': {'seconds': 2}})
        self.ctx = plan.Context[DesktopData](
            values, NO_OPTIONS, transport.Refusing(), Path(tempfile.mkdtemp()), log=lambda line: None
        )
        wled = wled_api.Wled(self.wled.http_address)
        self.ctx.data.update(
            prysm=prysm_api.Prysm(self.prysm.url),
            wled=wled,
            local_address='127.0.0.1',
            fps=30,
            clip_edges=True,
            wled_snapshot=wled_api.snapshot(wled),
        )


class DesktopChecks(Devices):
    def run_steps(self, names: list[str]) -> dict[str, plan.Outcome]:
        steps = [s for s in stage_desktop.steps(self.ctx.config, Options()) if s.name in names]
        self.assertEqual(len(steps), len(names))
        return {o.name: o for o in plan.execute('desktop', steps, self.ctx)}

    def test_the_checks_pass_against_a_well_behaved_prysm(self) -> None:
        names = [
            'desktop.processed',
            'desktop.wled-live',
            'desktop.fps',
            'desktop.colours',
            'desktop.device-native',
            'desktop.processed-again',
            'desktop.off',
            'desktop.dither',
            'desktop.brightness',
        ]
        outcomes = self.run_steps(names)
        for name in names:
            self.assertEqual(
                outcomes[name].status, plan.Status.PASS, (name, outcomes[name].detail, outcomes[name].evidence)
            )
        # The brightness check put WLED back.
        self.assertEqual(wled_api.differences(self.ctx.data['wled'], self.ctx.data['wled_snapshot']), {})
        dither = [
            r[2]['settings']['output']['dither'] for r in self.prysm.requests if r[:2] == ('PUT', '/api/v1/settings')
        ]
        self.assertEqual(dither, [False, True])

    def test_wled_without_ddp_fails_the_live_check(self) -> None:
        self.prysm.send_ddp = False
        outcomes = self.run_steps(['desktop.processed', 'desktop.wled-live'])
        self.assertEqual(outcomes['desktop.processed'].status, plan.Status.PASS)
        self.assertEqual(outcomes['desktop.wled-live'].status, plan.Status.FAIL)

    def test_wrong_edge_colours_fail(self) -> None:
        self.prysm.colours = lambda seconds: edges_event(
            {
                'top': ('blue', 'blue'),
                'right': ('green', 'blue'),
                'bottom': ('yellow', 'blue'),
                'left': ('red', 'yellow'),
            }
        )

        def colors(seconds: float, timed: bool) -> object:
            return prysm_api.Prysm(self.prysm.url).colors(1.0, timed)

        untyped(self.ctx.data['prysm']).colors = colors
        outcomes = self.run_steps(['desktop.colours'])
        self.assertEqual(outcomes['desktop.colours'].status, plan.Status.FAIL)
        self.assertIn('wrong colour: top shows blue, blue', str(outcomes['desktop.colours'].detail))
        self.assertEqual(untyped(outcomes['desktop.colours'].evidence)['samples'][0]['edges']['top'], 'blue, blue')

    def test_the_device_source_resolves_the_preset_like_the_portal(self) -> None:
        self.ctx.options = Options(source=Source.DEVICE)
        _, evidence = stage_desktop.source(self.ctx)
        self.assertEqual(untyped(evidence)['resolved'], {'width': 1280, 'height': 720, 'fps': 30})
        self.assertEqual(
            self.prysm.settings['capture'],
            {'kind': 'device', 'path': 'unique-id-1', 'width': 1280, 'height': 720, 'fps': 30},
        )


class GammaCheck(Devices):
    """desktop.gamma reads WLED's gamma fields as Prysm parses them, and changes nothing."""

    def gamma(self) -> plan.Outcome:
        step = next(s for s in stage_desktop.steps(self.ctx.config, Options()) if s.name == 'desktop.gamma')
        return plan.execute('desktop', [step], self.ctx)[0]

    def test_a_wled_16_configuration_passes_and_is_left_alone(self) -> None:
        before = json.dumps(self.wled.cfg, sort_keys=True)
        outcome = self.gamma()
        self.assertEqual(outcome.status, plan.Status.PASS, (outcome.detail, outcome.evidence))
        self.assertEqual(outcome.detail, 'WLED 16.0.1-fake: colour gamma 2.2, brightness gamma off')
        self.assertEqual(
            outcome.evidence,
            {'ver': '16.0.1-fake', 'if.live.no-gc': False, 'light.gc.col': 2.2, 'light.gc.bri': 1, 'light.gc.val': 2.2},
        )
        self.assertEqual((json.dumps(self.wled.cfg, sort_keys=True), self.wled.posts), (before, []))

    def test_realtime_gamma_off_is_described_as_linear(self) -> None:
        untyped(self.wled.cfg)['if']['live']['no-gc'] = True
        self.assertIn('realtime gamma correction off (linear)', str(self.gamma().detail))

    def test_each_missing_wrong_or_out_of_range_field_fails_by_name(self) -> None:
        cases = [
            (('if', 'live', 'no-gc'), None, 'if.live.no-gc is missing: Prysm would take realtime'),
            (('if', 'live', 'no-gc'), 0, 'if.live.no-gc is 0, not true or false: Prysm would refuse'),
            (('light', 'gc', 'col'), None, 'light.gc.col is missing: Prysm would take the colour gamma as unknown'),
            (('light', 'gc', 'col'), '2.2', "light.gc.col is '2.2', not a number: Prysm would refuse"),
            (('light', 'gc', 'col'), True, 'light.gc.col is True, not a number'),
            (('light', 'gc', 'col'), 3.5, 'light.gc.col is 3.5, outside 0.1–3: Prysm would take the colour'),
            (('light', 'gc', 'col'), 0.05, 'light.gc.col is 0.05, outside 0.1–3'),
            (('light', 'gc', 'bri'), None, 'light.gc.bri is missing: Prysm would take the brightness gamma as off'),
            (('light', 'gc', 'bri'), [1], 'light.gc.bri is [1], not a number: Prysm would refuse'),
            (('light', 'gc', 'bri'), 0, 'light.gc.bri is 0, outside 0.1–3: Prysm would take the brightness'),
            (('light', 'gc'), 2.2, 'light or light.gc is not an object: Prysm would refuse'),
            (('if', 'live'), None, 'if.live is missing or not an object: Prysm would refuse'),
        ]
        for path, value, message in cases:
            cfg = json.loads(json.dumps(self.wled.cfg))
            parent = cfg
            for key in path[:-1]:
                parent = parent[key]
            if value is None:
                del parent[path[-1]]
            else:
                parent[path[-1]] = value
            _, problems = wled_api.gamma_fields(cfg)
            self.assertEqual(len(problems), 1, (path, value, problems))
            self.assertIn(message, problems[0], (path, value))
        untyped(self.wled.cfg)['light']['gc']['col'] = 4
        outcome = self.gamma()
        self.assertEqual(outcome.status, plan.Status.FAIL)
        self.assertIn('light.gc.col is 4, outside 0.1–3', str(outcome.detail))
        self.assertEqual(untyped(outcome.evidence)['light.gc.col'], 4)

    def test_the_dry_run_lists_it_as_a_read(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            run.main(['desktop', '--dry-run', '--config', str(EXAMPLE)])
        entry = output.getvalue().split('] desktop.gamma: ')[1].split('\n[')[0]
        self.assertIn('auto  ] desktop.gamma', output.getvalue())
        self.assertIn('- GET /json/cfg (read-only)', entry)
        self.assertIn('not that Prysm applied the value', entry)


class FakeFeed:
    """A feed the runner controls: shows the clip, registers its stop, and settles before each colour check."""

    CLEANUPS: ClassVar[tuple[str, ...]] = ('the fake feed stopped',)

    def __init__(self) -> None:
        self.calls: list[object] = []

    def preflight(self) -> Never:
        self.calls.append('preflight')
        raise plan.Failed('the fake feed misses a tool')

    def how(self) -> str:
        return 'the fake feed shows the clip'

    def actions(self) -> list[str]:
        return ['show the clip on the fake feed']

    def commands(self) -> list[Command]:
        return []

    def show(self, ctx: Bench, clip: str) -> tuple[str, object]:
        self.calls.append(('show', clip))
        ctx.cleanup('desktop.fake-feed-stop', 'Fake feed stopped', lambda: self.calls.append('stop') or 'stopped')
        return 'showing', None

    def settled(self) -> None:
        self.calls.append('settled')


class TheFeed(Devices):
    """The stage shows the clip on the configured feed, and only for a capture card."""

    def setUp(self) -> None:
        super().setUp()
        self.feed = FakeFeed()
        configured = feed.configured
        feed.configured = lambda config: self.feed
        self.addCleanup(setattr, feed, 'configured', configured)

    def names(self, options: 'Options') -> list[str]:
        return [s.name for s in stage_desktop.steps(self.ctx.config, options)]

    def test_a_capture_card_gets_the_clip_through_the_feed(self) -> None:
        options = Options(source=Source.DEVICE)
        names = self.names(options)
        self.assertLess(names.index('desktop.clip'), names.index('desktop.feed'))
        self.assertLess(names.index('desktop.feed'), names.index('desktop.start'))
        step = next(s for s in stage_desktop.steps(self.ctx.config, options) if s.name == 'desktop.feed')
        self.assertEqual((step.mode, step.how), ('setup', 'the fake feed shows the clip'))
        self.assertEqual(
            stage_desktop.cleanups(self.ctx.config, options),
            stage_desktop.CLEANUPS[:1] + FakeFeed.CLEANUPS + stage_desktop.CLEANUPS[1:],
        )
        self.ctx.data['clip'] = 'RUN_DIR/edges.mp4'
        outcomes = plan.execute('desktop', [step], self.ctx)
        self.assertEqual(
            [(o.name, o.status) for o in outcomes],
            [('desktop.feed', plan.Status.PASS), ('desktop.fake-feed-stop', plan.Status.PASS)],
        )
        self.assertEqual(self.feed.calls, [('show', 'RUN_DIR/edges.mp4'), 'stop'])
        self.assertIs(self.ctx.data['feed'], self.feed)

    def test_preflight_checks_the_feed_only_for_a_capture_card(self) -> None:
        self.ctx.config.leds.top = 1
        self.ctx.options = Options(source=Source.DEVICE)
        with self.assertRaisesRegex(plan.Failed, 'fake feed misses'):
            stage_desktop.preflight(self.ctx)
        self.ctx.options = Options(source=Source.FILE)
        try:
            stage_desktop.preflight(self.ctx)
        except plan.Failed as error:  # only when this computer has no FFmpeg
            self.assertIn('FFmpeg', str(error))
        self.assertEqual(self.feed.calls, ['preflight'])

    def test_the_file_source_ignores_the_feed(self) -> None:
        names = self.names(Options(source=Source.FILE))
        self.assertNotIn('desktop.feed', names)
        self.assertIn('desktop.clip', names)
        self.assertEqual(stage_desktop.cleanups(self.ctx.config, Options()), stage_desktop.CLEANUPS)
        self.assertIsNone(stage_desktop._feed(self.ctx.config, Options()))

    def test_the_display_check_does_not_use_the_feed(self) -> None:
        options = Options(source=Source.DEVICE)
        options.display_check = True
        names = self.names(options)
        self.assertNotIn('desktop.clip', names)
        step = next(s for s in stage_desktop.steps(self.ctx.config, options) if s.name == 'desktop.feed')
        self.assertEqual(step.mode, 'skip')
        self.assertIn('--display-check', step.how)
        self.assertIn('desktop.display-check', names)

    def test_without_a_feed_the_card_is_only_not_black(self) -> None:
        feed.configured = lambda config: None
        options = Options(source=Source.DEVICE)
        self.assertNotIn('desktop.clip', self.names(options))
        step = next(s for s in stage_desktop.steps(self.ctx.config, options) if s.name == 'desktop.feed')
        self.assertEqual(step.mode, 'skip')
        self.assertIn('not black', step.how)
        self.assertEqual(stage_desktop.cleanups(self.ctx.config, options), stage_desktop.CLEANUPS)
        self.ctx.data['clip_edges'] = False
        detail, _ = stage_desktop.colours(self.ctx)
        self.assertIn('not black', detail)

    def test_colour_checks_wait_for_the_feed(self) -> None:
        self.ctx.data['feed'] = self.feed

        def colors(seconds: float, timed: bool) -> object:
            return timeline()

        untyped(self.ctx.data['prysm']).colors = colors
        detail, _ = stage_desktop.colours(self.ctx)
        self.assertIn('steps in order', detail)
        stage_desktop.release(self.ctx)
        self.assertEqual(self.feed.calls, ['settled', 'settled'])


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

    def fails(self, samples: Sequence[tuple[float, ColorsEvent]], message: str) -> plan.Failed:
        with self.assertRaisesRegex(plan.Failed, message) as failed:
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


class FakeWledBehaviour(Devices):
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
        self.assertNotEqual(wled_api.differences(wled, saved), {})
        self.assertEqual(wled_api.restore(wled, saved), {})


class StreamVersion(unittest.TestCase):
    def test_the_stand_in_answers_another_version(self) -> None:
        stand_in = stream_version.OtherVersion(0, host='127.0.0.1').start()
        self.addCleanup(stand_in.close)
        status, protocol = stream_version.ask(f'http://127.0.0.1:{stand_in.port}', stream_version.CURRENT)
        self.assertEqual((status, protocol), (426, stream_version.OTHER))
        self.assertEqual(stand_in.requests, 1)


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


class Modes(unittest.TestCase):
    @staticmethod
    def span(lo: int, hi: int, step: int = 1) -> Span:
        return {'min': lo, 'max': hi, 'step': step}

    def test_exact_then_smallest_wide_covering_size(self) -> None:
        wide: CaptureMode = {
            'width': self.span(176, 1920, 2),
            'height': self.span(144, 1080, 2),
            'fps': self.span(1, 30),
        }
        self.assertEqual(
            modes.resolve([wide], modes.PRESETS[Preset.BALANCED]), {'width': 640, 'height': 360, 'fps': 30}
        )
        self.assertIsNone(modes.resolve([wide], modes.PRESETS[Preset.SMOOTH]))
        fixed: list[CaptureMode] = [
            {'width': self.span(800, 800), 'height': self.span(600, 600), 'fps': self.span(30, 30)},
            {'width': self.span(1280, 1280), 'height': self.span(720, 720), 'fps': self.span(30, 30)},
            {'width': self.span(1920, 1920), 'height': self.span(1080, 1080), 'fps': self.span(30, 30)},
        ]
        self.assertEqual(
            modes.resolve(fixed, modes.PRESETS[Preset.BALANCED]), {'width': 1280, 'height': 720, 'fps': 30}
        )
        self.assertEqual(modes.describe(fixed[:1]), ['800 x 600 @ 30'])


class Configuration(unittest.TestCase):
    def test_the_example_parses(self) -> None:
        values = config.load(EXAMPLE)
        self.assertEqual(values.tv.ssh, 'root@' + values.tv.address)
        self.assertEqual(values.sweep.pairs()[0], (320, 180, 30))

    def test_mistakes_are_refused(self) -> None:
        cases: tuple[dict[str, object], ...] = (
            {'desktop': {'sekonds': 3}},
            {'wled': {'address': 'http://wled'}},
            {'tv': {'flavours': ['fat']}},
            {'desktop': {'seconds': '3'}},
            {'sweep': {'sizes': ['big']}},
            {'receiver': {'url': 'http://x/; reboot'}},
            {'leds': {'top': -1}},
            {'tv': {'ssh': '-oProxyCommand=sh'}},
            {'tv': {'address': 'tv\n'}},
        )
        for data in cases:
            with self.assertRaises(config.ConfigError, msg=data):
                config.parse(data)


class Options:
    def __init__(self, source: Source = Source.FILE) -> None:
        self.source = source
        self.source_path: str | None = None
        self.no_build, self.display_check = True, False
        self.sweep, self.tv_is_free = False, False


class Plan(unittest.TestCase):
    def context(self) -> plan.Context[Any]:
        return plan.Context[Any](
            config.parse({}), Options(), transport.Refusing(), Path(tempfile.mkdtemp()), log=lambda line: None
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

    def test_a_grouped_setup_runs_only_for_its_selected_checks(self) -> None:
        installed: list[str] = []
        steps: list[plan.Step[Any]] = [
            plan.Step('tv.full.install', 'I', 'h', Mode.SETUP, lambda c: installed.append('full'), group=Flavour.FULL),
            plan.Step('tv.full.soak', 'S', 'h', Mode.AUTO, lambda c: None, group=Flavour.FULL),
            plan.Step('tv.slim.install', 'I', 'h', Mode.SETUP, lambda c: installed.append('slim'), group=Flavour.SLIM),
            plan.Step('tv.slim.soak', 'S', 'h', Mode.AUTO, lambda c: None, group=Flavour.SLIM),
        ]
        plan.execute('tv', steps, self.context(), only=['tv.slim.*'])
        self.assertEqual(installed, ['slim'])

    def test_a_failed_setup_blocks_but_cleanups_run_and_the_report_says_so(self) -> None:
        ctx = self.context()
        restored: list[bool] = []

        def setup(ctx: Bench) -> Never:
            ctx.cleanup('a.restore', 'Restore', lambda: restored.append(True) or 'restored')
            raise plan.Failed('no device', {'why': 'unplugged'})

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

    def test_every_stage_prints_its_plan(self) -> None:
        for stage, expected in (
            ('desktop', 'desktop.sigterm'),
            ('tv', 'tv.slim.uninstall'),
            ('eyes', 'eyes.cold-boot'),
        ):
            code, text = self.dry(stage)
            self.assertEqual(code, 0)
            self.assertIn(expected, text)
            self.assertIn('nothing is executed', text)
        code, text = self.dry('tv', '--sweep')
        self.assertIn('TV$ timeout 120 luna-send -i luna://com.webos.appInstallService/dev/install', text)
        self.assertIn('tv.full.sweep', text)

    def test_the_tv_stage_needs_the_owners_word(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stderr(output):
            self.assertEqual(run.main(['tv', '--config', str(EXAMPLE)]), 2)
        self.assertIn('--tv-is-free', output.getvalue())


if __name__ == '__main__':
    unittest.main()
