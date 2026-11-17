"""A feed that casts: the generated clip served from this computer and played by a Cast receiver at an address.

`Cast` serves the clip on the address that routes to the receiver
(`host.local_address`), that one file with byte ranges as players ask for
them, and plays it there in the default media receiver on port 8009; it
registers what stops both again. `feed_chromecast` adds the Chromecast with
Google TV on the capture card's input (ADB); `TvCast` is the TV's own Cast
receiver, which asks a person on the TV to accept the first cast from this
computer. `CastPlayer` is the only code that imports PyChromecast, and only
when it connects, so a run without a feed never loads it and the tests use a
fake in its place.
"""

from collections.abc import Callable
import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
from pathlib import Path
import re
import threading
import time
from typing import TYPE_CHECKING, ClassVar, Protocol, cast

from prysm_acceptance import host
from prysm_acceptance.checks import Answer, Ask, Bench, Failed, Skipped, ask_person
from prysm_acceptance.commands import Command
from prysm_acceptance.config import CastFeed

if TYPE_CHECKING:
    import pychromecast

CONTENT_TYPE = 'video/mp4'
CAST_PORT = 8009
CONNECT_SECONDS = 20
PLAY_SECONDS = 30
_RANGE = re.compile(r'bytes=([0-9]*)-([0-9]*)')
INSTALL_HINT = 'PyChromecast is not installed: run the runner with uv run --project scripts/acceptance acceptance ...'
ACCEPT = (
    'The TV did not start the clip cast to it. If it shows a prompt to allow the cast from this computer, accept '
    'it with the remote, then answer y; skip goes on without the clip (the colour check is skipped).'
)


class Player(Protocol):
    """What plays the clip: `CastPlayer`, or a test's fake."""

    def connect(self, seconds: float) -> str: ...
    def play(self, url: str) -> None: ...
    def status(self) -> tuple[str | None, str | None]: ...
    def close(self) -> str: ...


class NotPlaying(RuntimeError):
    """The player did not report PLAYING in time."""


def stopped(receiver: str) -> tuple[str, ...]:
    """What the end of the run undoes for a cast to `receiver`, in that order, for the dry run."""
    return (f'{receiver}: media stopped, the default media receiver quit, disconnected', 'the clip server stopped')


class Cast:
    """The clip cast to the receiver at `address`: `show` plays it and registers its stops. A subclass names
    the receiver and prepares it (`reach`)."""

    RECEIVER: ClassVar[str] = 'the receiver'  # in messages and the dry run
    CLEANUPS: ClassVar[tuple[str, ...]] = stopped(RECEIVER)
    player_factory: Callable[[str], Player] | None = None  # `CastPlayer` unless a test gives a fake

    def __init__(self, settings: CastFeed, address: str) -> None:
        self.settings = settings
        self.address = address  # the receiver's, or its stand-in in the dry run until `reach` finds it
        self.playing: tuple[Player, float] | None = None  # (player, when it reported PLAYING)

    def preflight(self) -> None:
        if importlib.util.find_spec('pychromecast') is None:
            raise Failed(INSTALL_HINT)

    def how(self) -> str:
        return (
            f'{self.RECEIVER} plays the clip (PLAYING within {PLAY_SECONDS} s); colour checks first wait '
            f'{self.settings.settle_seconds:g} s for the player overlay to clear'
        )

    def actions(self) -> list[str]:
        return [
            f'serve http://THIS_COMPUTER:{self.settings.serve_port}/edges.mp4 (that file only) on the address '
            f'that routes to {self.RECEIVER}',
            f'cast to {self.address}:{CAST_PORT}: default media receiver plays the clip URL (video/mp4)',
        ]

    def commands(self) -> list[Command]:
        return []

    def reach(self, ctx: Bench, evidence: dict[str, object]) -> str:
        """The receiver ready for the cast, and its address."""
        return self.address

    def until_playing(self, player: Player) -> None:
        """Returns once the player reports PLAYING."""
        wait_playing(player, PLAY_SECONDS)

    def show(self, ctx: Bench, clip: str) -> tuple[str, object]:
        """Serve the clip to the receiver and play it there.

        `Skipped` when the clip is not shown but nothing failed (`TvCast`: nobody accepted the cast)."""
        evidence: dict[str, object] = {}
        address = self.reach(ctx, evidence)
        evidence['address'] = address
        # Bound by the constructor; started before its cleanup, as shutdown() waits for a running server.
        server = ClipServer(clip, host.local_address(address), self.settings.serve_port).start()
        ctx.cleanup('feed.server', 'Clip server stopped', server.close)
        player: Player = (self.player_factory or CastPlayer)(address)
        ctx.cleanup('feed.stop', 'Cast stopped', player.close)
        evidence['url'] = server.url
        try:
            evidence['device'] = player.connect(CONNECT_SECONDS)
            player.play(server.url)
            self.until_playing(player)
        except Skipped as error:
            raise Skipped(str(error), evidence) from error
        except Exception as error:  # PyChromecast's own errors, a timeout, or the receiver giving up
            raise Failed(f'casting to {address}: {type(error).__name__}: {error}', evidence) from error
        finally:
            evidence['clip_requests'] = list(server.requests)
        self.playing = (player, time.monotonic())
        return f'{server.url} playing on {self.RECEIVER} {evidence["device"]}', evidence

    def settled(self) -> None:
        """Wait out the player's overlay; the clip must still be playing (after `show`)."""
        player, since = cast('tuple[Player, float]', self.playing)
        left = since + self.settings.settle_seconds - time.monotonic()
        if left > 0:
            time.sleep(left)
        state, reason = player.status()
        if state != 'PLAYING':
            raise Failed(
                f'the clip is no longer playing on {self.RECEIVER}: {state}' + (f', {reason}' if reason else '')
            )


class TvCast(Cast):
    """`[tv.feed.cast]`: the clip cast to the TV's own Cast receiver at the host of `[tv] target`.

    The TV asks a person to allow the first cast from this computer; until then it reports no player state and
    does not fetch the clip. So when PLAYING does not come in time, a person is asked (`ask`) to accept the
    prompt on the TV, and the runner waits again; a skip, or nobody to ask, is `Skipped`. Nothing is accepted
    for the person."""

    RECEIVER = 'the TV'
    CLEANUPS: ClassVar[tuple[str, ...]] = stopped(RECEIVER)

    def __init__(self, settings: CastFeed, address: str) -> None:
        super().__init__(settings, address)
        self.ask: Ask = ask_person

    def how(self) -> str:
        return (
            f'{super().how()}; without PLAYING in time a person is asked to accept the cast on the TV (y: wait '
            f'{PLAY_SECONDS} s more; skip or no terminal: the clip is not shown and the colour check is skipped)'
        )

    def until_playing(self, player: Player) -> None:
        try:
            wait_playing(player, PLAY_SECONDS)
            return
        except NotPlaying as error:
            waited = str(error)
        answer, note = self.ask(ACCEPT, (Answer.YES, Answer.SKIP))
        if answer != Answer.YES:
            raise Skipped(f'the clip is not shown: {waited}, and the cast was not accepted ({note or answer})')
        wait_playing(player, PLAY_SECONDS)


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


class CastPlayer:
    """One Cast receiver reached by address (no discovery), playing a URL in the default media receiver.

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
        """The receiver `connect` reached; play and status come after it."""
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
    """Wait until the player reports PLAYING: `NotPlaying` after `seconds`, a RuntimeError at once when the
    receiver gives up on the clip."""
    deadline = time.monotonic() + seconds
    seen = None
    while True:
        seen = player.status()
        state, reason = seen
        if state == 'PLAYING':
            return
        # Other idle reasons may still describe the media played before this one.
        if state == 'IDLE' and reason == 'ERROR':
            raise RuntimeError('the receiver could not play the clip: IDLE, ERROR')
        if time.monotonic() >= deadline:
            raise NotPlaying(f'the receiver did not report PLAYING within {seconds:g} s (last: {seen})')
        sleep(0.25)
