"""What the runner's tests share: colours as a capture shows them, a seam's Prysm API, and typing helpers.

The host cells are tested against the real prysm-headless on loopback
(test_acceptance_host.py); `FakePrysm` only answers a seam's start.
"""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
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


class FakePrysm:
    """`GET /api/v1/state` and `PUT /api/v1/settings` on loopback, as a seam's start uses them; the saved
    settings come back in the state. Prysm listens on `listen`."""

    def __init__(self) -> None:
        self.saved: list[object] = []
        settings: dict[str, object] = {}
        saved = self.saved

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
