"""The Chromecast feed: the clip server on loopback, showing the clip with a fake Chromecast and recorded ADB,
its settings and its plan."""

from collections.abc import Iterable, Sequence
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import time
from typing import cast
import unittest
import urllib.error
import urllib.request

from acceptance_fakes import NO_OPTIONS, transcript_of, untyped

from prysm_acceptance import config, feed, feed_chromecast, plan, run, stage_desktop, transport
from prysm_acceptance.config import ChromecastFeed
from prysm_acceptance.options import Source
from prysm_acceptance.plan import Context
from prysm_acceptance.stage_data import DesktopData
from prysm_acceptance.transport import Result

# The runner's project: scripts/acceptance.
ACCEPTANCE = Path(__file__).resolve().parents[1]

EXAMPLE = ACCEPTANCE / 'config.example.toml'
SERIAL = '0123456789ABCD'
CLIP = bytes(range(256)) * 40  # 10240 bytes standing in for edges.mp4
# A player state, or a (state, idle reason) pair.
State = str | tuple[str, str | None]


def fetch(url: str, method: str = 'GET', headers: dict[str, str] | None = None) -> tuple[int, dict[str, str], bytes]:
    request = urllib.request.Request(url, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(request, timeout=5) as reply:
            return reply.status, dict(reply.headers), reply.read()
    except urllib.error.HTTPError as error:
        with error:
            return error.code, dict(error.headers), error.read()


def clip_file() -> Path:
    path = Path(tempfile.mkdtemp()) / 'edges.mp4'
    path.write_bytes(CLIP)
    return path


class ClipServer(unittest.TestCase):
    def setUp(self) -> None:
        self.server = feed_chromecast.ClipServer(clip_file(), '127.0.0.1', 0).start()
        self.addCleanup(self.server.close)

    def test_serves_the_clip_as_mp4(self) -> None:
        status, headers, body = fetch(self.server.url)
        self.assertEqual((status, body), (200, CLIP))
        self.assertEqual(headers['Content-Type'], 'video/mp4')
        self.assertEqual(headers['Accept-Ranges'], 'bytes')
        status, headers, body = fetch(self.server.url, 'HEAD')
        self.assertEqual((status, headers['Content-Length'], body), (200, str(len(CLIP)), b''))

    def test_answers_byte_ranges(self) -> None:
        status, headers, body = fetch(self.server.url, headers={'Range': 'bytes=100-199'})
        self.assertEqual((status, body), (206, CLIP[100:200]))
        self.assertEqual(headers['Content-Range'], f'bytes 100-199/{len(CLIP)}')
        self.assertEqual(fetch(self.server.url, headers={'Range': 'bytes=10000-'})[2], CLIP[10000:])
        self.assertEqual(fetch(self.server.url, headers={'Range': 'bytes=-40'})[2], CLIP[-40:])
        self.assertEqual(fetch(self.server.url, headers={'Range': 'bytes=10000-99999'})[2], CLIP[10000:])
        status, headers, _ = fetch(self.server.url, headers={'Range': 'bytes=99999-'})
        self.assertEqual((status, headers['Content-Range']), (416, f'bytes */{len(CLIP)}'))

    def test_serves_nothing_else(self) -> None:
        base = self.server.url.rsplit('/', 1)[0]
        for path in ('/', '/config.toml', '/../edges.mp4', '/edges.mp4/x', '/settings.json'):
            self.assertEqual(fetch(base + path)[0], 404, path)


class FakeChromecast:
    """Stands in for `feed_chromecast.Chromecast`: fetches the clip as a player would and reports a state."""

    def __init__(
        self, address: str, states: Iterable[State] = ('BUFFERING', 'PLAYING'), fail: Exception | None = None
    ) -> None:
        self.address = address
        self.states: list[State] = list(states)
        self.fail = fail
        self.calls: list[str] = []
        self.fetched: tuple[int, dict[str, str], bytes] | None = None
        self.made_for: list[str] = []

    def connect(self, seconds: float) -> str:
        self.calls.append('connect')
        if self.fail:
            raise self.fail
        return 'Fake Chromecast'

    def play(self, url: str) -> None:
        self.calls.append('play')
        self.fetched = fetch(url, headers={'Range': 'bytes=0-'})

    def status(self) -> tuple[str | None, str | None]:
        state = self.states.pop(0) if len(self.states) > 1 else self.states[0]
        if isinstance(state, tuple):
            return state
        return state, ('ERROR' if state == 'IDLE' else None)

    def close(self) -> str:
        self.calls.append('close')
        return 'closed'


def context(
    entries: Sequence[tuple[str, Result]] = (), **settings: object
) -> tuple[Context[DesktopData], feed_chromecast.Feed]:
    """A context with recorded ADB and the Chromecast feed its settings describe (any free port)."""
    values = config.parse(
        {
            'wled': {'address': 'wled.example'},
            'leds': {'top': 1},
            'feed': {'chromecast': {'adb_serial': SERIAL, **settings}},
        }
    )
    cast('ChromecastFeed', values.feed).serve_port = 0
    ctx = Context[DesktopData](
        values, NO_OPTIONS, transport.Transcript(entries), Path(tempfile.mkdtemp()), log=lambda line: None
    )
    return ctx, cast('feed_chromecast.Feed', feed.configured(values))


# Recorded from `ip -f inet -o addr show wlan0` on the Chromecast (address replaced by a documentation one).
WLAN0 = (
    '15: wlan0    inet 192.0.2.44/24 brd 192.0.2.255 scope global wlan0\\       '
    'valid_lft forever preferred_lft forever\n'
)
# The whole exchange of a good run; the Chromecast's address is loopback here, as the clip server binds to it.
ADB_OK = [
    ('adb.get-state', Result(0, 'device\n')),
    ('adb.stay-on-get', Result(0, '0\n')),
    ('adb.stay-on-put', Result(0, '')),
    ('adb.wake', Result(0, '')),
    ('adb.wakefulness', Result(0, '  mWakefulness=Awake\n')),
    ('adb.wifi-address', Result(0, WLAN0.replace('192.0.2.44', '127.0.0.1'))),
    ('adb.stay-on-put', Result(0, '')),
]


class Show(unittest.TestCase):
    def show(
        self, ctx: Context[DesktopData], chromecast: feed_chromecast.Feed, player: FakeChromecast
    ) -> list[plan.Outcome]:
        made: list[str] = []
        chromecast.player_factory = lambda address: made.append(address) or player
        clip = str(clip_file())
        step = plan.Step[DesktopData]('desktop.feed', 'F', 'h', plan.Mode.SETUP, lambda ctx: chromecast.show(ctx, clip))
        outcomes = plan.execute('desktop', [step], ctx)
        player.made_for = made
        return outcomes

    def test_plays_the_clip_and_undoes_everything_in_order(self) -> None:
        ctx, chromecast = context(ADB_OK)
        player = FakeChromecast('127.0.0.1')
        outcomes = self.show(ctx, chromecast, player)
        self.assertEqual(
            [(o.name, o.status) for o in outcomes],
            [
                ('desktop.feed', plan.Status.PASS),
                ('desktop.feed-stop', plan.Status.PASS),
                ('desktop.feed-server', plan.Status.PASS),
                ('desktop.feed-awake', plan.Status.PASS),
            ],
        )
        self.assertEqual(player.made_for, ['127.0.0.1'])  # the address ADB read
        self.assertEqual(untyped(player.fetched)[0], 206)
        self.assertEqual(untyped(player.fetched)[1]['Content-Type'], 'video/mp4')
        puts = [c.argv[-1] for c in transcript_of(ctx).calls if c.template == 'adb.stay-on-put']
        self.assertEqual(
            puts, ['settings put global stay_on_while_plugged_in 7', 'settings put global stay_on_while_plugged_in 0']
        )
        self.assertEqual(player.calls, ['connect', 'play', 'close'])
        self.assertIs(untyped(chromecast.playing)[0], player)

    def test_unreachable_adb_says_what_to_do_and_changes_nothing(self) -> None:
        ctx, chromecast = context([('adb.get-state', Result(1, '', "error: device '0123456789ABCD' not found"))])
        player = FakeChromecast('127.0.0.1')
        outcomes = self.show(ctx, chromecast, player)
        self.assertEqual(outcomes[0].status, plan.Status.FAIL)
        self.assertIn('USB debugging', str(outcomes[0].detail))
        self.assertIn('adb connect', str(outcomes[0].detail))
        self.assertIn('feed.chromecast.adb_serial', str(outcomes[0].detail))
        self.assertEqual(len(outcomes), 1)  # nothing to undo
        self.assertEqual(player.calls, [])

    def test_a_failed_cast_still_runs_every_cleanup(self) -> None:
        ctx, chromecast = context(ADB_OK)
        player = FakeChromecast('127.0.0.1', fail=TimeoutError('no answer on 8009'))
        outcomes = self.show(ctx, chromecast, player)
        self.assertEqual(outcomes[0].status, plan.Status.FAIL)
        self.assertIn('no answer on 8009', str(outcomes[0].detail))
        self.assertEqual(
            [o.name for o in outcomes[1:]], ['desktop.feed-stop', 'desktop.feed-server', 'desktop.feed-awake']
        )
        self.assertEqual(player.calls, ['connect', 'close'])
        self.assertIsNone(chromecast.playing)

    def test_no_wifi_address_fails_and_puts_the_setting_back(self) -> None:
        entries = [*ADB_OK[:5], ('adb.wifi-address', Result(0, '')), ADB_OK[-1]]
        ctx, chromecast = context(entries)
        player = FakeChromecast('x')
        outcomes = self.show(ctx, chromecast, player)
        self.assertEqual(outcomes[0].status, plan.Status.FAIL)
        self.assertIn('no Wi-Fi address', str(outcomes[0].detail))
        self.assertEqual([o.name for o in outcomes[1:]], ['desktop.feed-awake'])
        self.assertEqual(player.calls, [])

    def test_the_wifi_address_is_parsed_strictly(self) -> None:
        ctx, _ = context([('adb.wifi-address', Result(0, WLAN0))])
        self.assertEqual(feed_chromecast.wifi_address(ctx, SERIAL), '192.0.2.44')
        for output in ('', 'Device "wlan0" does not exist.\n', '15: wlan0    inet 999.0.2.44/24 brd x\n'):
            ctx, _ = context([('adb.wifi-address', Result(0, output))])
            with self.assertRaisesRegex(plan.Failed, 'no Wi-Fi address'):
                feed_chromecast.wifi_address(ctx, SERIAL)

    def test_a_clip_the_receiver_rejects_fails_at_once(self) -> None:
        ctx, chromecast = context(ADB_OK)
        player = FakeChromecast('127.0.0.1', states=('IDLE',))
        start = time.monotonic()
        outcomes = self.show(ctx, chromecast, player)
        self.assertLess(time.monotonic() - start, 5)
        self.assertEqual(outcomes[0].status, plan.Status.FAIL)
        self.assertIn('IDLE, ERROR', str(outcomes[0].detail))
        self.assertEqual(player.calls[-1], 'close')

    def test_the_previous_media_ending_is_not_a_failure(self) -> None:
        player = FakeChromecast('x', states=(('IDLE', 'INTERRUPTED'), ('BUFFERING', None), ('PLAYING', None)))
        feed_chromecast.wait_playing(player, 5, sleep=lambda s: None)

    def test_waiting_for_playing_is_bounded(self) -> None:
        player = FakeChromecast('x', states=('BUFFERING',))
        with self.assertRaisesRegex(RuntimeError, 'did not report PLAYING within 0.2 s'):
            feed_chromecast.wait_playing(player, 0.2, sleep=lambda s: None)


class Settled(unittest.TestCase):
    """Before a colour check: the player's overlay has gone and the clip still plays."""

    def setUp(self) -> None:
        _, self.chromecast = context(settle_seconds=0.3)
        self.player = FakeChromecast('x', states=('PLAYING',))
        self.chromecast.playing = (self.player, time.monotonic())

    def test_waits_out_the_overlay(self) -> None:
        start = time.monotonic()
        self.chromecast.settled()
        self.assertGreaterEqual(time.monotonic() - start, 0.25)
        start = time.monotonic()
        self.chromecast.settled()  # once only
        self.assertLess(time.monotonic() - start, 0.1)

    def test_a_clip_no_longer_playing_fails(self) -> None:
        self.player.states = ['IDLE']
        with self.assertRaisesRegex(plan.Failed, 'no longer playing on the Chromecast: IDLE, ERROR'):
            self.chromecast.settled()


class Settings(unittest.TestCase):
    def test_the_section_is_checked(self) -> None:
        for data in (
            {'address': '192.0.2.44'},
            {'adb_serial': '-H evil'},
            {},
            {'adb_serial': SERIAL, 'serve_port': 0},
            {'adb_serial': SERIAL, 'settle_seconds': '5'},
            {'adb_serial': SERIAL, 'settle_seconds': 61},
            {'adb_serial': SERIAL, 'adb_port': 5555},
        ):
            with self.assertRaises(config.ConfigError, msg=data):
                config.parse({'feed': {'chromecast': data}})
        values = config.parse({'feed': {'chromecast': {'adb_serial': '192.0.2.60:37000'}}})
        self.assertEqual(
            (untyped(values.feed).adb_serial, untyped(values.feed).serve_port, untyped(values.feed).settle_seconds),
            ('192.0.2.60:37000', 18792, 5.0),
        )
        self.assertIsInstance(feed.configured(values), feed_chromecast.Feed)

    def test_without_a_feed_there_is_none(self) -> None:
        self.assertIsNone(config.parse({}).feed)
        self.assertIsNone(feed.configured(config.parse({})))

    def test_other_feeds_and_the_old_section_are_refused(self) -> None:
        cases: tuple[dict[str, object], ...] = (
            {'chromecast': {'adb_serial': SERIAL}},
            {'feed': {'firestick': {}}},
            {'feed': 'chromecast'},
            {'feed': {'chromecast': 'yes'}},
        )
        for data in cases:
            with self.assertRaises(config.ConfigError, msg=data):
                config.parse(data)

    def test_the_example_names_a_chromecast(self) -> None:
        self.assertEqual(untyped(config.load(EXAMPLE).feed).adb_serial, SERIAL)


class Options:
    source, source_path, no_build, display_check, sweep, tv_is_free = Source.DEVICE, None, True, False, False, False


class FeedPlan(unittest.TestCase):
    def dry(self, *arguments: str) -> tuple[int, str]:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = run.main(['desktop', *arguments, '--dry-run', '--config', str(EXAMPLE)])
        return code, output.getvalue()

    def test_dry_run_lists_the_step_and_what_it_undoes(self) -> None:
        code, text = self.dry('--source', 'device')
        self.assertEqual(code, 0, text)
        self.assertIn('desktop.feed: Picture into the capture card', text)
        self.assertIn('the Chromecast plays the clip (PLAYING within 30 s)', text)
        self.assertIn('serve http://THIS_COMPUTER:18792/edges.mp4', text)
        self.assertIn('-movflags +faststart', text)
        self.assertIn(f'$ adb -s {SERIAL} get-state', text)
        self.assertIn(f"$ adb -s {SERIAL} shell 'settings put global stay_on_while_plugged_in 7'", text)
        self.assertIn(f"$ adb -s {SERIAL} shell 'ip -f inet -o addr show wlan0'", text)
        cleanups = text[text.index('Always at the end') :].splitlines()[1:6]
        self.assertEqual(
            [line.split()[1:3] for line in cleanups],
            [
                ['prysm-headless', 'stopped'],
                ['the', 'Chromecast:'],
                ['the', 'clip'],
                ['the', "Chromecast's"],
                ['WLED', 'restored'],
            ],
        )

    def test_the_file_source_never_mentions_it(self) -> None:
        code, text = self.dry()
        self.assertEqual(code, 0, text)
        self.assertNotIn('Chromecast', text)
        self.assertNotIn('$ adb', text)

    def test_the_old_flag_is_gone(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as exited:
            run.main(['desktop', '--source', 'device', '--cast', '--dry-run'])
        self.assertEqual(exited.exception.code, 2)

    def test_nothing_imports_pychromecast_until_it_connects(self) -> None:
        stage_desktop.steps(config.load(EXAMPLE), Options())
        self.assertNotIn('pychromecast', sys.modules)


if __name__ == '__main__':
    unittest.main()
