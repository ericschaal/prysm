"""`desktop --cast`: the clip server on loopback, the cast step with a fake Chromecast and recorded ADB, the plan."""
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

import cast  # noqa: E402
import config  # noqa: E402
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
        self.server = cast.ClipServer(clip_file(), '127.0.0.1', 0).start()
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
    """Stands in for `cast.Chromecast`: fetches the clip as a player would and reports a state."""

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


class Options:
    source, source_path, no_build, display_check, cast = 'device', None, True, False, True


def context(entries=(), **cast_settings):
    values = config.parse({'wled': {'address': 'wled.example'}, 'leds': {'top': 1},
                           'chromecast': {'adb_serial': SERIAL, **cast_settings}})
    values.chromecast.serve_port = 0  # any free port
    ctx = plan.Context(values, Options(), transport.Transcript(entries), Path(tempfile.mkdtemp()),
                       log=lambda line: None)
    ctx.data['clip'] = str(clip_file())
    return ctx


# Recorded from `ip -f inet -o addr show wlan0` on the Chromecast (address replaced by a documentation one).
WLAN0 = ('15: wlan0    inet 192.0.2.44/24 brd 192.0.2.255 scope global wlan0\\       '
         'valid_lft forever preferred_lft forever\n')
# The whole exchange of a good run; the Chromecast's address is loopback here, as the clip server binds to it.
ADB_OK = [('adb.get-state', Result(0, 'device\n')), ('adb.stay-on-get', Result(0, '0\n')),
          ('adb.stay-on-put', Result(0, '')), ('adb.wake', Result(0, '')),
          ('adb.wakefulness', Result(0, '  mWakefulness=Awake\n')),
          ('adb.wifi-address', Result(0, WLAN0.replace('192.0.2.44', '127.0.0.1'))),
          ('adb.stay-on-put', Result(0, ''))]


class CastStep(unittest.TestCase):
    def run_step(self, ctx, player):
        made = []
        ctx.data['chromecast_factory'] = lambda address: made.append(address) or player
        outcomes = plan.execute('desktop', [plan.Step('desktop.chromecast', 'C', 'h', 'setup',
                                                      stage_desktop.chromecast)], ctx)
        player.made_for = made
        return outcomes

    def test_plays_the_clip_and_undoes_everything_in_order(self):
        ctx = context(ADB_OK)
        player = FakeChromecast('127.0.0.1')
        outcomes = self.run_step(ctx, player)
        self.assertEqual([(o.name, o.status) for o in outcomes],
                         [('desktop.chromecast', plan.PASS), ('desktop.cast-stop', plan.PASS),
                          ('desktop.clip-server', plan.PASS), ('desktop.chromecast-awake', plan.PASS)])
        self.assertEqual(player.made_for, ['127.0.0.1'])  # the address ADB read
        self.assertEqual(player.fetched[0], 206)
        self.assertEqual(player.fetched[1]['Content-Type'], 'video/mp4')
        puts = [c.argv[-1] for c in ctx.transport.calls if c.template == 'adb.stay-on-put']
        self.assertEqual(puts, ['settings put global stay_on_while_plugged_in 7',
                                'settings put global stay_on_while_plugged_in 0'])
        self.assertEqual(player.calls, ['connect', 'play', 'close'])

    def test_unreachable_adb_says_what_to_do_and_changes_nothing(self):
        ctx = context([('adb.get-state', Result(1, '', "error: device '0123456789ABCD' not found"))])
        player = FakeChromecast('127.0.0.1')
        outcomes = self.run_step(ctx, player)
        self.assertEqual(outcomes[0].status, plan.FAIL)
        self.assertIn('USB debugging', outcomes[0].detail)
        self.assertIn('adb connect', outcomes[0].detail)
        self.assertEqual(len(outcomes), 1)  # nothing to undo
        self.assertEqual(player.calls, [])

    def test_a_failed_cast_still_runs_every_cleanup(self):
        ctx = context(ADB_OK)
        player = FakeChromecast('127.0.0.1', fail=TimeoutError('no answer on 8009'))
        outcomes = self.run_step(ctx, player)
        self.assertEqual(outcomes[0].status, plan.FAIL)
        self.assertIn('no answer on 8009', outcomes[0].detail)
        self.assertEqual([o.name for o in outcomes[1:]],
                         ['desktop.cast-stop', 'desktop.clip-server', 'desktop.chromecast-awake'])
        self.assertEqual(player.calls, ['connect', 'close'])

    def test_no_wifi_address_fails_and_puts_the_setting_back(self):
        entries = ADB_OK[:5] + [('adb.wifi-address', Result(0, '')), ADB_OK[-1]]
        ctx = context(entries)
        player = FakeChromecast('x')
        outcomes = self.run_step(ctx, player)
        self.assertEqual(outcomes[0].status, plan.FAIL)
        self.assertIn('no Wi-Fi address', outcomes[0].detail)
        self.assertEqual([o.name for o in outcomes[1:]], ['desktop.chromecast-awake'])
        self.assertEqual(player.calls, [])

    def test_the_wifi_address_is_parsed_strictly(self):
        ctx = context([('adb.wifi-address', Result(0, WLAN0))])
        self.assertEqual(stage_desktop._wifi_address(ctx, SERIAL), '192.0.2.44')
        for output in ('', 'Device "wlan0" does not exist.\n', '15: wlan0    inet 999.0.2.44/24 brd x\n'):
            ctx = context([('adb.wifi-address', Result(0, output))])
            with self.assertRaisesRegex(plan.Failed, 'no Wi-Fi address'):
                stage_desktop._wifi_address(ctx, SERIAL)

    def test_a_clip_the_receiver_rejects_fails_at_once(self):
        ctx = context(ADB_OK)
        player = FakeChromecast('127.0.0.1', states=('IDLE',))
        start = time.monotonic()
        outcomes = self.run_step(ctx, player)
        self.assertLess(time.monotonic() - start, 5)
        self.assertEqual(outcomes[0].status, plan.FAIL)
        self.assertIn('IDLE, ERROR', outcomes[0].detail)
        self.assertEqual(player.calls[-1], 'close')

    def test_the_previous_media_ending_is_not_a_failure(self):
        player = FakeChromecast('x', states=(('IDLE', 'INTERRUPTED'), ('BUFFERING', None), ('PLAYING', None)))
        cast.wait_playing(player, 5, sleep=lambda s: None)

    def test_waiting_for_playing_is_bounded(self):
        player = FakeChromecast('x', states=('BUFFERING',))
        with self.assertRaisesRegex(RuntimeError, 'did not report PLAYING within 0.2 s'):
            cast.wait_playing(player, 0.2, sleep=lambda s: None)


class CastColours(unittest.TestCase):
    """While casting, the colour check compares the edges with the clip's, after the overlay has gone."""

    def setUp(self):
        self.ctx = context(settle_seconds=0.3)
        self.player = FakeChromecast('x', states=('PLAYING',))
        self.ctx.data.update(clip_edges=True, cast={'player': self.player, 'playing_since': time.monotonic()})
        stage_desktop._start_processed = lambda ctx, seconds=15: {}
        self.addCleanup(lambda: setattr(stage_desktop, '_start_processed', START_PROCESSED))

    def colours(self, event):
        self.ctx.data['prysm'] = type('Prysm', (), {'colors': lambda self, seconds: [event]})()
        return stage_desktop.colours(self.ctx)

    def test_compares_with_the_clip_after_settling(self):
        start = time.monotonic()
        detail, evidence = self.colours({'type': 'colors', 'top': [[240, 20, 30]] * 8, 'right': [[30, 220, 40]] * 8,
                                         'bottom': [[20, 30, 230]] * 8, 'left': [[230, 230, 30]] * 8})
        self.assertGreaterEqual(time.monotonic() - start, 0.25)
        self.assertIn('clip', detail)
        self.assertEqual(evidence['measured']['left']['seen'], 'yellow')

    def test_a_card_showing_something_else_fails(self):
        with self.assertRaises(plan.Failed):
            self.colours({'type': 'colors', 'top': [[40, 40, 40]] * 8, 'right': [[40, 40, 40]] * 8,
                          'bottom': [[40, 40, 40]] * 8, 'left': [[40, 40, 40]] * 8})

    def test_a_clip_no_longer_playing_fails(self):
        self.player.states = ['IDLE']
        with self.assertRaisesRegex(plan.Failed, 'no longer playing'):
            self.colours({'type': 'colors'})


START_PROCESSED = stage_desktop._start_processed


class CastPlan(unittest.TestCase):
    def dry(self, *arguments, config_path=EXAMPLE):
        output, errors = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
            try:
                code = run.main(['desktop', *arguments, '--dry-run', '--config', str(config_path)])
            except SystemExit as exit:
                code = exit.code
        return code, output.getvalue() + errors.getvalue()

    def test_the_cast_step_comes_after_the_clip_and_before_the_source(self):
        names = [s.name for s in stage_desktop.steps(config.load(EXAMPLE), Options())]
        self.assertLess(names.index('desktop.clip'), names.index('desktop.chromecast'))
        self.assertLess(names.index('desktop.chromecast'), names.index('desktop.start'))
        self.assertLess(names.index('desktop.start'), names.index('desktop.source'))
        self.assertIn('desktop.release', names)
        Options.cast = False
        try:
            names = [s.name for s in stage_desktop.steps(config.load(EXAMPLE), Options())]
        finally:
            Options.cast = True
        self.assertNotIn('desktop.chromecast', names)
        self.assertNotIn('desktop.clip', names)

    def test_dry_run_lists_the_step_and_what_it_undoes(self):
        code, text = self.dry('--source', 'device', '--cast')
        self.assertEqual(code, 0, text)
        self.assertIn('desktop.chromecast', text)
        self.assertIn('serve http://THIS_COMPUTER:18792/edges.mp4', text)
        self.assertIn('-movflags +faststart', text)
        self.assertIn('default media receiver quit', text)
        self.assertIn(f'$ adb -s {SERIAL} get-state', text)
        self.assertIn(f"$ adb -s {SERIAL} shell 'settings put global stay_on_while_plugged_in 7'", text)
        self.assertIn(f"$ adb -s {SERIAL} shell 'ip -f inet -o addr show wlan0'", text)
        self.assertIn('stay_on_while_plugged_in put back', text)

    def test_cast_without_an_adb_serial_fails_in_preflight(self):
        values = config.parse({'wled': {'address': 'wled.example'}, 'leds': {'top': 1}})
        ctx = plan.Context(values, Options(), transport.Refusing(), Path(tempfile.mkdtemp()), log=lambda line: None)
        with self.assertRaisesRegex(plan.Failed, 'adb_serial'):
            stage_desktop.preflight(ctx)

    def test_cast_is_refused_with_the_file_source_or_the_display_check(self):
        for arguments in (['--cast'], ['--source', 'file:clip.mp4', '--cast'],
                          ['--source', 'device', '--cast', '--display-check']):
            code, text = self.dry(*arguments)
            self.assertEqual(code, 2, arguments)
            self.assertIn('--cast', text)

    def test_the_config_checks_the_chromecast_section(self):
        for data in ({'address': '192.0.2.44'}, {'adb_serial': '-H evil'}, {'serve_port': 0},
                     {'settle_seconds': '5'}, {'adb_port': 5555}):
            with self.assertRaises(config.ConfigError, msg=data):
                config.parse({'chromecast': data})
        self.assertEqual(config.parse({'chromecast': {'adb_serial': '192.0.2.60:37000'}}).chromecast.adb_serial,
                         '192.0.2.60:37000')

    def test_nothing_imports_pychromecast_until_it_connects(self):
        self.assertNotIn('pychromecast', sys.modules)


if __name__ == '__main__':
    unittest.main()
