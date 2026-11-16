"""The bench's controlled feed: a Chromecast with Google TV on the capture card's HDMI input.

`Feed` shows the generated clip there and registers what stops it again. Over
ADB it keeps the Chromecast awake and reads its Wi-Fi address; it serves the
clip from this computer and plays it with PyChromecast. The server listens on
the address that routes to the Chromecast (`host.local_address`) and serves
that one file, with byte ranges as players ask for them. `Chromecast` is the
only code that imports PyChromecast, and only when it connects, so the stages
without a feed never load it and the tests use a fake in its place.
"""

from collections.abc import Callable
import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
from pathlib import Path
import re
import shutil
import threading
import time
from typing import TYPE_CHECKING, ClassVar, Protocol, cast

from prysm_acceptance import commands, host
from prysm_acceptance.commands import Command
from prysm_acceptance.config import ChromecastFeed
from prysm_acceptance.plan import Bench, Failed

if TYPE_CHECKING:
    import pychromecast

CONTENT_TYPE = 'video/mp4'
CAST_PORT = 8009
CONNECT_SECONDS = 20
PLAY_SECONDS = 30
_RANGE = re.compile(r'bytes=([0-9]*)-([0-9]*)')
INSTALL_HINT = 'PyChromecast is not installed: run the runner with uv run --project scripts/acceptance acceptance ...'
ADB_HINT = (
    'ADB does not reach the Chromecast {serial}: plug it into this computer by USB with USB debugging '
    'on (Settings > System > Developer options) and allow this computer on the TV; for wireless '
    'debugging, run adb connect HOST:PORT yourself and give HOST:PORT as feed.chromecast.adb_serial'
)


class Player(Protocol):
    """What plays the clip: `Chromecast`, or a test's fake."""

    def connect(self, seconds: float) -> str: ...
    def play(self, url: str) -> None: ...
    def status(self) -> tuple[str | None, str | None]: ...
    def close(self) -> str: ...


class Feed:
    """The Chromecast `[feed.chromecast]` names: `show` plays the clip and registers its stops."""

    # What the end of the run undoes, in that order, for the dry run.
    CLEANUPS: ClassVar[tuple[str, ...]] = (
        'the Chromecast: media stopped, the default media receiver quit, disconnected',
        'the clip server stopped',
        "the Chromecast's stay_on_while_plugged_in put back to the value read before "
        "(adb -s SERIAL shell 'settings put global stay_on_while_plugged_in VALUE')",
    )
    player_factory: Callable[[str], Player] | None = None  # `Chromecast` unless a test gives a fake

    def __init__(self, settings: ChromecastFeed) -> None:
        self.settings = settings
        self.playing: tuple[Player, float] | None = None  # (player, when it reported PLAYING)

    def preflight(self) -> None:
        if not shutil.which(commands.adb_get_state(self.settings.adb_serial).argv[0]):
            raise Failed('the Chromecast feed needs adb (Android platform tools) on PATH')
        if importlib.util.find_spec('pychromecast') is None:
            raise Failed(INSTALL_HINT)

    def how(self) -> str:
        return (
            f'the Chromecast plays the clip (PLAYING within {PLAY_SECONDS} s); colour checks first wait '
            f'{self.settings.settle_seconds:g} s for the player overlay to clear'
        )

    def actions(self) -> list[str]:
        serial = self.settings.adb_serial
        return [
            f'ADB {serial}: get-state is device; stay_on_while_plugged_in read, set to {commands.STAY_ON} '
            '(put back at the end); wake; mWakefulness=Awake; the wlan0 address read',
            f'serve http://THIS_COMPUTER:{self.settings.serve_port}/edges.mp4 (that file only) on the '
            'address that routes to the Chromecast',
            f'cast to CHROMECAST:{CAST_PORT}: default media receiver plays the clip URL (video/mp4)',
        ]

    def commands(self) -> list[Command]:
        serial = self.settings.adb_serial
        return [
            commands.adb_get_state(serial),
            commands.adb_stay_on_get(serial),
            commands.adb_stay_on_put(serial, commands.STAY_ON),
            commands.adb_wake(serial),
            commands.adb_wakefulness(serial),
            commands.adb_wifi_address(serial),
        ]

    def show(self, ctx: Bench, clip: str) -> tuple[str, object]:
        """Keep the Chromecast awake (ADB), serve the clip to it and play it there."""
        settings = self.settings
        evidence: dict[str, object] = {'keep_awake': keep_awake(ctx, settings.adb_serial)}
        address = wifi_address(ctx, settings.adb_serial)
        evidence['address'] = address
        # Bound by the constructor; started before its cleanup, as shutdown() waits for a running server.
        server = ClipServer(clip, host.local_address(address), settings.serve_port).start()
        ctx.cleanup('desktop.feed-server', 'Clip server stopped', server.close)
        player: Player = (self.player_factory or Chromecast)(address)
        ctx.cleanup('desktop.feed-stop', 'Cast stopped', player.close)
        evidence['url'] = server.url
        try:
            evidence['device'] = player.connect(CONNECT_SECONDS)
            player.play(server.url)
            wait_playing(player, PLAY_SECONDS)
        except Exception as error:  # PyChromecast's own errors, a timeout, or the receiver giving up
            evidence['clip_requests'] = list(server.requests)
            raise Failed(f'casting to {address}: {type(error).__name__}: {error}', evidence) from error
        evidence['clip_requests'] = list(server.requests)
        self.playing = (player, time.monotonic())
        return f'{server.url} playing on the Chromecast {evidence["device"]}', evidence

    def settled(self) -> None:
        """Wait out the player's overlay; the clip must still be playing (after `show`)."""
        player, since = cast('tuple[Player, float]', self.playing)
        left = since + self.settings.settle_seconds - time.monotonic()
        if left > 0:
            time.sleep(left)
        state, reason = player.status()
        if state != 'PLAYING':
            raise Failed(
                f'the clip is no longer playing on the Chromecast: {state}' + (f', {reason}' if reason else '')
            )


def keep_awake(ctx: Bench, serial: str) -> dict[str, object]:
    """Check ADB, keep the screen on while powered (put back at the end) and wake it."""
    state = ctx.run(commands.adb_get_state(serial), check=False)
    if state.stdout.strip() != 'device':
        raise Failed(ADB_HINT.format(serial=serial), (state.stdout + state.stderr).strip())
    read = ctx.run(commands.adb_stay_on_get(serial)).stdout.strip()
    # A setting never written reads as null; the default is 0.
    before = 0 if read == 'null' else int(read) if read.isdigit() else -1
    if not 0 <= before <= 15:
        raise Failed(f'stay_on_while_plugged_in reads {read!r}, not 0 to 15')

    def restore() -> str:
        ctx.run(commands.adb_stay_on_put(serial, before))
        return f'stay_on_while_plugged_in {before}'

    ctx.cleanup('desktop.feed-awake', 'Chromecast keep-awake put back', restore)
    ctx.run(commands.adb_stay_on_put(serial, commands.STAY_ON))
    ctx.run(commands.adb_wake(serial))
    power = ''
    for _ in range(10):
        power = ctx.run(commands.adb_wakefulness(serial), check=False).stdout.strip()
        if 'mWakefulness=Awake' in power:
            return {'stay_on_before': before, 'power': power}
        time.sleep(0.5)
    raise Failed('the Chromecast did not wake: mWakefulness is not Awake after 5 s', power)


def wifi_address(ctx: Bench, serial: str) -> str:
    """The Chromecast's IPv4 address on wlan0, read over ADB."""
    line = ctx.run(commands.adb_wifi_address(serial)).stdout
    found = re.search(r'\binet ((?:[0-9]{1,3}\.){3}[0-9]{1,3})/', line)
    if not found or not all(int(part) <= 255 for part in found[1].split('.')):
        raise Failed(
            'the Chromecast has no Wi-Fi address: connect it to the same network as this computer', line.strip()
        )
    return found[1]


class ClipServer:
    """Serves one file at `/<its name>` on `address:port`; anything else is 404."""

    def __init__(self, path: str | Path, address: str, port: int) -> None:
        path = Path(path)
        self.name = path.name
        self.data = path.read_bytes()  # a few MB; read once
        self.requests: list[tuple[str, str, str | None]] = []
        self.server = ThreadingHTTPServer((address, port), self._handler())
        self.server.daemon_threads = True

    @property
    def url(self) -> str:
        address, port = self.server.server_address[:2]
        return f'http://{address}:{port}/{self.name}'

    def start(self) -> 'ClipServer':
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        return self

    def close(self) -> str:
        self.server.shutdown()
        self.server.server_close()
        return f'stopped after {len(self.requests)} requests'

    def _handler(self) -> type[BaseHTTPRequestHandler]:
        clip = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: object) -> None:
                pass

            def do_HEAD(self) -> None:
                self._serve(body=False)

            def do_GET(self) -> None:
                self._serve(body=True)

            def _serve(self, body: bool) -> None:
                clip.requests.append((self.command, self.path, self.headers.get('Range')))
                if self.path.split('?')[0] != '/' + clip.name:
                    self.send_error(404)
                    return
                size = len(clip.data)
                first, last = 0, size - 1
                status = 200
                wanted = _RANGE.fullmatch(self.headers.get('Range', '').strip())
                if wanted and (wanted[1] or wanted[2]):
                    if wanted[1]:
                        first = int(wanted[1])
                        last = min(int(wanted[2]), size - 1) if wanted[2] else size - 1
                    else:  # the last N bytes
                        first = max(size - int(wanted[2]), 0)
                    if first > last or first >= size:
                        self.send_response(416)
                        self.send_header('Content-Range', f'bytes */{size}')
                        self.send_header('Content-Length', '0')
                        self.end_headers()
                        return
                    status = 206
                self.send_response(status)
                self.send_header('Content-Type', CONTENT_TYPE)
                self.send_header('Accept-Ranges', 'bytes')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.send_header('Content-Length', str(last - first + 1))
                if status == 206:
                    self.send_header('Content-Range', f'bytes {first}-{last}/{size}')
                self.end_headers()
                if body:
                    # Players drop connections they no longer need.
                    with contextlib.suppress(BrokenPipeError, ConnectionResetError):
                        self.wfile.write(clip.data[first : last + 1])

        return Handler


class Chromecast:
    """One Chromecast reached by address (no discovery), playing a URL in the default media receiver.

    `status()` is (player state, idle reason), e.g. ('PLAYING', None) or
    ('IDLE', 'ERROR'). `close()` undoes whatever was done, also half-way."""

    def __init__(self, address: str) -> None:
        self.address = address
        self.cast: pychromecast.Chromecast | None = None
        self.launched = False

    def connect(self, seconds: float) -> str:
        try:
            import pychromecast  # noqa: PLC0415 -- loaded only when casting: a run without the feed never loads it (tested)
        except ImportError as error:
            raise RuntimeError(INSTALL_HINT) from error
        # (address, port, uuid, model, name): the uuid and names come from the device itself.
        self.cast = pychromecast.get_chromecast_from_host(
            (self.address, CAST_PORT, None, None, None),  # pyright: ignore[reportArgumentType] -- PyChromecast takes None for the uuid (filled from the device) though its annotation says UUID
            timeout=seconds,
        )
        self.cast.wait(timeout=seconds)
        return self.cast.cast_info.friendly_name or self.address

    def _connected(self) -> 'pychromecast.Chromecast':
        """The Chromecast `connect` reached; play and status come after it."""
        return cast('pychromecast.Chromecast', self.cast)

    def play(self, url: str) -> None:
        self.launched = True
        self._connected().media_controller.play_media(  # pyright: ignore[reportUnknownMemberType] -- an unannotated parameter in PyChromecast
            url, CONTENT_TYPE, title='Prysm acceptance edges', stream_type='BUFFERED'
        )

    def status(self) -> tuple[str | None, str | None]:
        status = self._connected().media_controller.status
        state = cast('str | None', status.player_state)  # pyright: ignore[reportUnknownMemberType] -- PyChromecast leaves this attribute's type partly unannotated
        return state, status.idle_reason

    def close(self) -> str:
        if self.cast is None:
            return 'not connected'
        try:
            if self.launched:
                try:
                    if self.cast.media_controller.status.media_session_id is not None:
                        self.cast.media_controller.stop()
                finally:
                    self.cast.quit_app()
        finally:
            self.cast.disconnect(timeout=10)
        return 'media stopped, receiver quit' if self.launched else 'disconnected'


def wait_playing(player: Player, seconds: float, sleep: Callable[[float], object] = time.sleep) -> None:
    """Wait until the player reports PLAYING; a RuntimeError saying what it reported otherwise."""
    deadline = time.monotonic() + seconds
    seen = None
    while True:
        seen = player.status()
        state, reason = seen
        if state == 'PLAYING':
            return
        # Other idle reasons may still describe the media played before this one.
        if state == 'IDLE' and reason == 'ERROR':
            raise RuntimeError('the Chromecast could not play the clip: IDLE, ERROR')
        if time.monotonic() >= deadline:
            raise RuntimeError(f'the Chromecast did not report PLAYING within {seconds} s (last: {seen})')
        sleep(0.25)
