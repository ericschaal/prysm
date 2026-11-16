"""The Chromecast feed: the clip server on loopback, showing the clip with a fake Chromecast and recorded ADB,
its settings and its plan."""
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import time
import unittest
import urllib.error
import urllib.request

ACCEPTANCE = Path(__file__).resolve().parents[1] / 'acceptance'
sys.path.insert(0, str(ACCEPTANCE))

import config  # noqa: E402
import feed  # noqa: E402
import feed_chromecast  # noqa: E402
import plan  # noqa: E402
import run  # noqa: E402
import stage_desktop  # noqa: E402
import transport  # noqa: E402
from transport import Result  # noqa: E402

EXAMPLE = ACCEPTANCE / 'config.example.toml'
SERIAL = '0123456789ABCD'
CLIP = bytes(range(256)) * 40  # 10240 bytes standing in for edges.mp4


def fetch(url, method='GET', headers=None):
    request = urllib.request.Request(url, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(request, timeout=5) as reply:
            return reply.status, dict(reply.headers), reply.read()
    except urllib.error.HTTPError as error:
        with error:
            return error.code, dict(error.headers), error.read()


def clip_file():
    path = Path(tempfile.mkdtemp()) / 'edges.mp4'
    path.write_bytes(CLIP)
    return path


class ClipServer(unittest.TestCase):
    def setUp(self):
        self.server = feed_chromecast.ClipServer(clip_file(), '127.0.0.1', 0).start()
        self.addCleanup(self.server.close)

    def test_serves_the_clip_as_mp4(self):
        status, headers, body = fetch(self.server.url)
        self.assertEqual((status, body), (200, CLIP))
        self.assertEqual(headers['Content-Type'], 'video/mp4')
        self.assertEqual(headers['Accept-Ranges'], 'bytes')
        status, headers, body = fetch(self.server.url, 'HEAD')
        self.assertEqual((status, headers['Content-Length'], body), (200, str(len(CLIP)), b''))

    def test_answers_byte_ranges(self):
        status, headers, body = fetch(self.server.url, headers={'Range': 'bytes=100-199'})
        self.assertEqual((status, body), (206, CLIP[100:200]))
        self.assertEqual(headers['Content-Range'], f'bytes 100-199/{len(CLIP)}')
        self.assertEqual(fetch(self.server.url, headers={'Range': 'bytes=10000-'})[2], CLIP[10000:])
        self.assertEqual(fetch(self.server.url, headers={'Range': 'bytes=-40'})[2], CLIP[-40:])
        self.assertEqual(fetch(self.server.url, headers={'Range': 'bytes=10000-99999'})[2], CLIP[10000:])
        status, headers, _ = fetch(self.server.url, headers={'Range': 'bytes=99999-'})
        self.assertEqual((status, headers['Content-Range']), (416, f'bytes */{len(CLIP)}'))

    def test_serves_nothing_else(self):
        base = self.server.url.rsplit('/', 1)[0]
        for path in ('/', '/config.toml', '/../edges.mp4', '/edges.mp4/x', '/settings.json'):
            self.assertEqual(fetch(base + path)[0], 404, path)


class FakeChromecast:
    """Stands in for `feed_chromecast.Chromecast`: fetches the clip as a player would and reports a state."""

    def __init__(self, address, states=('BUFFERING', 'PLAYING'), fail=None):
        self.address = address
        self.states = list(states)
        self.fail = fail
        self.calls = []

    def connect(self, seconds):
        self.calls.append('connect')
        if self.fail:
            raise self.fail
        return 'Fake Chromecast'

    def play(self, url):
        self.calls.append('play')
        self.fetched = fetch(url, headers={'Range': 'bytes=0-'})

    def status(self):
        state = self.states.pop(0) if len(self.states) > 1 else self.states[0]
        if isinstance(state, tuple):
            return state
        return state, ('ERROR' if state == 'IDLE' else None)

    def close(self):
        self.calls.append('close')
        return 'closed'


def context(entries=(), **settings):
    """A context with recorded ADB and the Chromecast feed its settings describe (any free port)."""
    values = config.parse({'wled': {'address': 'wled.example'}, 'leds': {'top': 1},
                           'feed': {'chromecast': {'adb_serial': SERIAL, **settings}}})
    values.feed.serve_port = 0
    ctx = plan.Context(values, None, transport.Transcript(entries), Path(tempfile.mkdtemp()),
                       log=lambda line: None)
    return ctx, feed.configured(values)


# Recorded from `ip -f inet -o addr show wlan0` on the Chromecast (address replaced by a documentation one).
WLAN0 = ('15: wlan0    inet 192.0.2.44/24 brd 192.0.2.255 scope global wlan0\\       '
         'valid_lft forever preferred_lft forever\n')
# The whole exchange of a good run; the Chromecast's address is loopback here, as the clip server binds to it.
ADB_OK = [('adb.get-state', Result(0, 'device\n')), ('adb.stay-on-get', Result(0, '0\n')),
          ('adb.stay-on-put', Result(0, '')), ('adb.wake', Result(0, '')),
          ('adb.wakefulness', Result(0, '  mWakefulness=Awake\n')),
          ('adb.wifi-address', Result(0, WLAN0.replace('192.0.2.44', '127.0.0.1'))),
          ('adb.stay-on-put', Result(0, ''))]


class Show(unittest.TestCase):
    def show(self, ctx, chromecast, player):
        made = []
        chromecast.player_factory = lambda address: made.append(address) or player
        clip = str(clip_file())
        outcomes = plan.execute('desktop', [plan.Step('desktop.feed', 'F', 'h', 'setup',
                                                      lambda ctx: chromecast.show(ctx, clip))], ctx)
        player.made_for = made
        return outcomes

    def test_plays_the_clip_and_undoes_everything_in_order(self):
        ctx, chromecast = context(ADB_OK)
        player = FakeChromecast('127.0.0.1')
        outcomes = self.show(ctx, chromecast, player)
        self.assertEqual([(o.name, o.status) for o in outcomes],
                         [('desktop.feed', plan.PASS), ('desktop.feed-stop', plan.PASS),
                          ('desktop.feed-server', plan.PASS), ('desktop.feed-awake', plan.PASS)])
        self.assertEqual(player.made_for, ['127.0.0.1'])  # the address ADB read
        self.assertEqual(player.fetched[0], 206)
        self.assertEqual(player.fetched[1]['Content-Type'], 'video/mp4')
        puts = [c.argv[-1] for c in ctx.transport.calls if c.template == 'adb.stay-on-put']
        self.assertEqual(puts, ['settings put global stay_on_while_plugged_in 7',
                                'settings put global stay_on_while_plugged_in 0'])
        self.assertEqual(player.calls, ['connect', 'play', 'close'])
        self.assertIs(chromecast.playing[0], player)

    def test_unreachable_adb_says_what_to_do_and_changes_nothing(self):
        ctx, chromecast = context([('adb.get-state', Result(1, '', "error: device '0123456789ABCD' not found"))])
        player = FakeChromecast('127.0.0.1')
        outcomes = self.show(ctx, chromecast, player)
        self.assertEqual(outcomes[0].status, plan.FAIL)
        self.assertIn('USB debugging', outcomes[0].detail)
        self.assertIn('adb connect', outcomes[0].detail)
        self.assertIn('feed.chromecast.adb_serial', outcomes[0].detail)
        self.assertEqual(len(outcomes), 1)  # nothing to undo
        self.assertEqual(player.calls, [])

    def test_a_failed_cast_still_runs_every_cleanup(self):
        ctx, chromecast = context(ADB_OK)
        player = FakeChromecast('127.0.0.1', fail=TimeoutError('no answer on 8009'))
        outcomes = self.show(ctx, chromecast, player)
        self.assertEqual(outcomes[0].status, plan.FAIL)
        self.assertIn('no answer on 8009', outcomes[0].detail)
        self.assertEqual([o.name for o in outcomes[1:]],
                         ['desktop.feed-stop', 'desktop.feed-server', 'desktop.feed-awake'])
        self.assertEqual(player.calls, ['connect', 'close'])
        self.assertIsNone(chromecast.playing)

    def test_no_wifi_address_fails_and_puts_the_setting_back(self):
        entries = ADB_OK[:5] + [('adb.wifi-address', Result(0, '')), ADB_OK[-1]]
        ctx, chromecast = context(entries)
        player = FakeChromecast('x')
        outcomes = self.show(ctx, chromecast, player)
        self.assertEqual(outcomes[0].status, plan.FAIL)
        self.assertIn('no Wi-Fi address', outcomes[0].detail)
        self.assertEqual([o.name for o in outcomes[1:]], ['desktop.feed-awake'])
        self.assertEqual(player.calls, [])

    def test_the_wifi_address_is_parsed_strictly(self):
        ctx, _ = context([('adb.wifi-address', Result(0, WLAN0))])
        self.assertEqual(feed_chromecast.wifi_address(ctx, SERIAL), '192.0.2.44')
        for output in ('', 'Device "wlan0" does not exist.\n', '15: wlan0    inet 999.0.2.44/24 brd x\n'):
            ctx, _ = context([('adb.wifi-address', Result(0, output))])
            with self.assertRaisesRegex(plan.Failed, 'no Wi-Fi address'):
                feed_chromecast.wifi_address(ctx, SERIAL)

    def test_a_clip_the_receiver_rejects_fails_at_once(self):
        ctx, chromecast = context(ADB_OK)
        player = FakeChromecast('127.0.0.1', states=('IDLE',))
        start = time.monotonic()
        outcomes = self.show(ctx, chromecast, player)
        self.assertLess(time.monotonic() - start, 5)
        self.assertEqual(outcomes[0].status, plan.FAIL)
        self.assertIn('IDLE, ERROR', outcomes[0].detail)
        self.assertEqual(player.calls[-1], 'close')

    def test_the_previous_media_ending_is_not_a_failure(self):
        player = FakeChromecast('x', states=(('IDLE', 'INTERRUPTED'), ('BUFFERING', None), ('PLAYING', None)))
        feed_chromecast.wait_playing(player, 5, sleep=lambda s: None)

    def test_waiting_for_playing_is_bounded(self):
        player = FakeChromecast('x', states=('BUFFERING',))
        with self.assertRaisesRegex(RuntimeError, 'did not report PLAYING within 0.2 s'):
            feed_chromecast.wait_playing(player, 0.2, sleep=lambda s: None)


class Settled(unittest.TestCase):
    """Before a colour check: the player's overlay has gone and the clip still plays."""

    def setUp(self):
        _, self.chromecast = context(settle_seconds=0.3)
        self.player = FakeChromecast('x', states=('PLAYING',))
        self.chromecast.playing = (self.player, time.monotonic())

    def test_waits_out_the_overlay(self):
        start = time.monotonic()
        self.chromecast.settled()
        self.assertGreaterEqual(time.monotonic() - start, 0.25)
        start = time.monotonic()
        self.chromecast.settled()  # once only
        self.assertLess(time.monotonic() - start, 0.1)

    def test_a_clip_no_longer_playing_fails(self):
        self.player.states = ['IDLE']
        with self.assertRaisesRegex(plan.Failed, 'no longer playing on the Chromecast: IDLE, ERROR'):
            self.chromecast.settled()


class Settings(unittest.TestCase):
    def test_the_section_is_checked(self):
        for data in ({'address': '192.0.2.44'}, {'adb_serial': '-H evil'}, {},
                     {'adb_serial': SERIAL, 'serve_port': 0}, {'adb_serial': SERIAL, 'settle_seconds': '5'},
                     {'adb_serial': SERIAL, 'settle_seconds': 61}, {'adb_serial': SERIAL, 'adb_port': 5555}):
            with self.assertRaises(config.ConfigError, msg=data):
                config.parse({'feed': {'chromecast': data}})
        values = config.parse({'feed': {'chromecast': {'adb_serial': '192.0.2.60:37000'}}})
        self.assertEqual((values.feed.adb_serial, values.feed.serve_port, values.feed.settle_seconds),
                         ('192.0.2.60:37000', 18792, 5.0))
        self.assertIsInstance(feed.configured(values), feed_chromecast.Feed)

    def test_without_a_feed_there_is_none(self):
        self.assertIsNone(config.parse({}).feed)
        self.assertIsNone(feed.configured(config.parse({})))

    def test_other_feeds_and_the_old_section_are_refused(self):
        for data in ({'chromecast': {'adb_serial': SERIAL}}, {'feed': {'firestick': {}}}, {'feed': 'chromecast'},
                     {'feed': {'chromecast': 'yes'}}):
            with self.assertRaises(config.ConfigError, msg=data):
                config.parse(data)

    def test_the_example_names_a_chromecast(self):
        self.assertEqual(config.load(EXAMPLE).feed.adb_serial, SERIAL)


class Options:
    source, source_path, no_build, display_check = 'device', None, True, False


class FeedPlan(unittest.TestCase):
    def dry(self, *arguments):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = run.main(['desktop', *arguments, '--dry-run', '--config', str(EXAMPLE)])
        return code, output.getvalue()

    def test_dry_run_lists_the_step_and_what_it_undoes(self):
        code, text = self.dry('--source', 'device')
        self.assertEqual(code, 0, text)
        self.assertIn('desktop.feed: Picture into the capture card', text)
        self.assertIn('the Chromecast plays the clip (PLAYING within 30 s)', text)
        self.assertIn('serve http://THIS_COMPUTER:18792/edges.mp4', text)
        self.assertIn('-movflags +faststart', text)
        self.assertIn(f'$ adb -s {SERIAL} get-state', text)
        self.assertIn(f"$ adb -s {SERIAL} shell 'settings put global stay_on_while_plugged_in 7'", text)
        self.assertIn(f"$ adb -s {SERIAL} shell 'ip -f inet -o addr show wlan0'", text)
        cleanups = text[text.index('Always at the end'):].splitlines()[1:6]
        self.assertEqual([line.split()[1:3] for line in cleanups],
                         [['prysm-headless', 'stopped'], ['the', 'Chromecast:'], ['the', 'clip'],
                          ['the', "Chromecast's"], ['WLED', 'restored']])

    def test_the_file_source_never_mentions_it(self):
        code, text = self.dry()
        self.assertEqual(code, 0, text)
        self.assertNotIn('Chromecast', text)
        self.assertNotIn('$ adb', text)

    def test_the_old_flag_is_gone(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as exit:
            run.main(['desktop', '--source', 'device', '--cast', '--dry-run'])
        self.assertEqual(exit.exception.code, 2)

    def test_nothing_imports_pychromecast_until_it_connects(self):
        stage_desktop.steps(config.load(EXAMPLE), Options())
        self.assertNotIn('pychromecast', sys.modules)


if __name__ == '__main__':
    unittest.main()
