"""What the runner's tests share: colours as a capture shows them, a seam's Prysm API, a colours stream, and
typing helpers.

The host cells are tested against the real prysm-headless on loopback
(test_acceptance_host.py); `FakePrysm` only answers a seam's start.
"""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
import time
from typing import Any, cast

from prysm_acceptance.checks import Bench
from prysm_acceptance.prysm_api import ColorsEvent, Edge
from prysm_acceptance.transport import Result, Transcript


def untyped(value: object) -> Any:
    """A value as a test reads it (evidence, mostly): of whatever shape the code under test gave."""
    return value


def transcript_of(ctx: Bench) -> Transcript:
    """The recorded transport a test's bench runs on."""
    return cast('Transcript', ctx.transport)


def ok(stdout: str = '') -> Result:
    """A remote command's success, printing `stdout`."""
    return Result(0, stdout)


# A target with what the colour check's reader runs: python3 and `timeout` found.
TOOLS = ('device.reader-tools', ok('/usr/bin/python3\n/usr/bin/timeout\n'))


class FakePrysm:
    """`GET /api/v1/state`, `PUT /api/v1/settings` and `PUT /api/v1/target` on loopback, as a seam's start and a
    restore use them; the saved settings come back in the state, the targets put are in `targets`. Prysm listens
    on `listen`."""

    def __init__(self) -> None:
        self.saved: list[object] = []
        self.targets: list[object] = []
        settings: dict[str, object] = {}
        saved, targets = self.saved, self.targets

        class Handler(BaseHTTPRequestHandler):
            def reply(self, value: object) -> None:
                body = json.dumps(value).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self) -> None:
                self.reply({'target': 'off', 'session': {'state': 'stopped'}, 'settings': settings, 'revision': 1})

            def do_PUT(self) -> None:
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                if self.path == '/api/v1/target':
                    targets.append(body['target'])
                    self.send_response(204)
                    self.end_headers()
                    return
                saved.append(body)
                settings.clear()
                settings.update(body['settings'])
                self.reply({'revision': 2})

            def log_message(self, format: str, *args: object) -> None:
                pass

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.listen = f'127.0.0.1:{self.server.server_address[1]}'

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()


def sse(payload: object) -> bytes:
    """One server-sent event carrying `payload`, as Prysm's colours stream frames it."""
    return b'data: ' + json.dumps(payload).encode() + b'\n\n'


class FakeColoursStream:
    """`GET /api/v1/colors/stream` on loopback (`url`), framed as Prysm's server frames it: HTTP/1.1, chunked,
    each of `chunks` (seconds after the request, bytes; usually one `sse` event) one chunk at its time. Then, with
    `final`, the body's last chunk (`0`); either way the connection is held open until `close`, as a keep-alive
    connection is. Another `status` than 200 is answered with an empty body."""

    def __init__(self, chunks: list[tuple[float, bytes]], final: bool = False, status: int = 200) -> None:
        self.closing = threading.Event()
        closing = self.closing

        class Handler(BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def do_GET(self) -> None:
                self.close_connection = True  # one request per connection, once `close` lets it go
                self.send_response(status)
                self.send_header('Content-Type', 'text/event-stream')
                if status != 200:
                    self.send_header('Content-Length', '0')
                    self.end_headers()
                    return
                self.send_header('Transfer-Encoding', 'chunked')
                self.end_headers()
                start = time.monotonic()
                for at, data in chunks:
                    time.sleep(max(0.0, start + at - time.monotonic()))
                    self.wfile.write(f'{len(data):x}\r\n'.encode() + data + b'\r\n')
                    self.wfile.flush()
                if final:
                    self.wfile.write(b'0\r\n\r\n')
                    self.wfile.flush()
                closing.wait(30)

            def log_message(self, format: str, *args: object) -> None:
                pass

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f'http://127.0.0.1:{self.server.server_address[1]}/api/v1/colors/stream'

    def close(self) -> None:
        self.closing.set()
        self.server.shutdown()
        self.server.server_close()


# Named colours as a capture shifts them a little.
RGB = {
    'red': [235, 60, 40],
    'green': [50, 230, 60],
    'blue': [45, 70, 235],
    'yellow': [240, 235, 45],
    'gray': [70, 70, 70],
    'white': [245, 240, 250],
}


def edges_event(pairs: dict[Edge, tuple[str, ...]]) -> ColorsEvent:
    """A ColorsEvent with 8 entries per edge: four of each of its two named colours, in the stream's order."""
    event: ColorsEvent = {'type': 'colors'}
    for edge, (first, second) in pairs.items():
        event[edge] = [RGB[first]] * 4 + [RGB[second]] * 4
    return event
