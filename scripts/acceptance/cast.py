"""The generated clip on a Chromecast: a one-file HTTP server and a small seam over PyChromecast.

The Chromecast fetches the clip from this computer, so the server listens on
the address that routes to it (`host.local_address`) and serves that one
file, with byte ranges as players ask for them. `Chromecast` is the only code
that imports PyChromecast, and only when it connects, so the stages that do
not cast run with plain python3 and the tests use a fake in its place.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import re
import threading
import time

CONTENT_TYPE = 'video/mp4'
CAST_PORT = 8009
_RANGE = re.compile(r'bytes=([0-9]*)-([0-9]*)')
INSTALL_HINT = ('PyChromecast is not installed: run the runner with '
                'uv run --project scripts/acceptance scripts/acceptance/run.py ...')


class ClipServer:
    """Serves one file at `/<its name>` on `address:port`; anything else is 404."""

    def __init__(self, path, address, port):
        path = Path(path)
        self.name = path.name
        self.data = path.read_bytes()  # a few MB; read once
        self.requests = []
        self.server = ThreadingHTTPServer((address, port), self._handler())
        self.server.daemon_threads = True

    @property
    def url(self):
        address, port = self.server.server_address[:2]
        return f'http://{address}:{port}/{self.name}'

    def start(self):
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        return self

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        return f'stopped after {len(self.requests)} requests'

    def _handler(clip):
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_HEAD(self):
                self._serve(body=False)

            def do_GET(self):
                self._serve(body=True)

            def _serve(self, body):
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
                    try:
                        self.wfile.write(clip.data[first:last + 1])
                    except (BrokenPipeError, ConnectionResetError):
                        pass  # players drop connections they no longer need
        return Handler


class Chromecast:
    """One Chromecast reached by address (no discovery), playing a URL in the default media receiver.

    `status()` is (player state, idle reason), e.g. ('PLAYING', None) or
    ('IDLE', 'ERROR'). `close()` undoes whatever was done, also half-way."""

    def __init__(self, address):
        self.address = address
        self.cast = None
        self.launched = False

    def connect(self, seconds):
        try:
            import pychromecast
        except ImportError as error:
            raise RuntimeError(INSTALL_HINT) from error
        # (address, port, uuid, model, name): the uuid and names come from the device itself.
        self.cast = pychromecast.get_chromecast_from_host((self.address, CAST_PORT, None, None, None),
                                                          timeout=seconds)
        self.cast.wait(timeout=seconds)
        return self.cast.cast_info.friendly_name or self.address

    def play(self, url):
        self.launched = True
        self.cast.media_controller.play_media(url, CONTENT_TYPE, title='Prysm acceptance edges',
                                              stream_type='BUFFERED')

    def status(self):
        status = self.cast.media_controller.status
        return status.player_state, status.idle_reason

    def close(self):
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


def wait_playing(player, seconds, sleep=time.sleep):
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
