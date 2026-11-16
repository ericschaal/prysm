"""A stand-in Prysm portal for the acceptance runner's tests.

It answers the API calls the desktop checks make and, like Prysm, drives a
`FakeWled` (in-process): live mode and DDP while processed, `lor: 2` for
device effects, off for off, and reports WLED's brightness while it scales
the colours. `send_ddp = False` makes it forget DDP, as a broken output would.
While processed, its colours stream shows the stepping test clip, sending
only on change like Prysm; `colours(seconds)` says what it shows that long
after processing started.
"""

from collections.abc import Callable
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
import time
from typing import Any, cast

from prysm_acceptance import edges_clip
from prysm_acceptance.fake_wled import FakeWled
from prysm_acceptance.options import Options
from prysm_acceptance.plan import Bench
from prysm_acceptance.prysm_api import ColorsEvent, Edge, Span
from prysm_acceptance.transport import Transcript

# A JSON body as the fake reads and sends it (a test double: not typed further).
Body = Any
# The checks under test read no option.
NO_OPTIONS = cast('Options', None)


def untyped(value: object) -> Any:
    """A value as a test reads it (evidence, mostly): of whatever shape the code under test gave."""
    return value


def transcript_of(ctx: Bench) -> Transcript:
    """The recorded transport a test's context runs on."""
    return cast('Transcript', ctx.transport)


# Named colours as a capture shifts them a little.
RGB = {
    'red': [235, 60, 40],
    'green': [50, 230, 60],
    'blue': [45, 70, 235],
    'yellow': [240, 235, 45],
    'gray': [70, 70, 70],
}


def edges_event(pairs: dict[Edge, tuple[str, ...]]) -> ColorsEvent:
    """A ColorsEvent with 8 entries per edge: four of each of its two named colours, in the stream's order."""
    event: ColorsEvent = {'type': 'colors'}
    for edge, (first, second) in pairs.items():
        event[edge] = [RGB[first]] * 4 + [RGB[second]] * 4
    return event


def stepping(seconds: float) -> ColorsEvent:
    """What the clip shows `seconds` after it started."""
    return edges_event(edges_clip.expected(int(seconds // edges_clip.STEP_SECONDS) % edges_clip.STEPS))


class FakePrysm:
    def __init__(self, wled: FakeWled, settings: Body) -> None:
        self.wled = wled
        self.target = 'off'
        self.settings = settings
        self.revision = 41
        self.send_ddp = True
        self.colours: Callable[[float], ColorsEvent] = stepping
        self.processed_at = time.monotonic()
        self.requests: list[tuple[Any, ...]] = []
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), self._handler())
        self.server.daemon_threads = True
        self.running = True

    @property
    def url(self) -> str:
        return f'http://127.0.0.1:{self.server.server_address[1]}'

    def start(self) -> 'FakePrysm':
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        threading.Thread(target=self._ddp, daemon=True).start()
        return self

    def close(self) -> None:
        self.running = False
        self.server.shutdown()
        self.server.server_close()

    def _ddp(self) -> None:
        while self.running:
            if self.target == 'processed' and self.send_ddp:
                with self.wled.lock:
                    self.wled.ddp_at = time.monotonic()
                    self.wled.ddp_from = '127.0.0.1'
            time.sleep(0.1)

    def state(self) -> Body:
        state: Body = {
            'target': self.target,
            'settings': self.settings,
            'revision': self.revision,
            'failure': None,
            'surface_failures': [],
            'session': {'state': {'off': 'stopped', 'processed': 'running', 'device_native': 'native'}[self.target]},
        }
        if self.target == 'processed':
            state['input'] = {'fps': 30.0, 'receiving': True, 'sent_fps': 30.0, 'latency': {'output_ms': 1}}
            with self.wled.lock:
                cfg: Body = self.wled.cfg
                force_max = cfg['if']['live']['maxbri']
                bri: Body = self.wled.state['bri']
            if not force_max:
                state['output_brightness'] = {'percent': round(bri * 100 / 255)}
        return state

    def set_target(self, target: str) -> None:
        if target == 'processed' and self.target != 'processed':
            self.processed_at = time.monotonic()
        self.target = target
        body: Body = {
            'processed': {'on': True, 'lor': 0, 'live': True, 'tt': 0},
            'device_native': {'on': True, 'lor': 2, 'tt': 0},
            'off': {'on': False, 'lor': 2, 'tt': 0},
        }[target]
        if target != 'processed':
            self.wled.post_state({'live': False})
        self.wled.post_state(body)

    def _handler(self) -> type[BaseHTTPRequestHandler]:
        prysm = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: object) -> None:
                pass

            def _reply(self, status: int, body: Body = None) -> None:
                data = b'' if body is None else json.dumps(body).encode()
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def _body(self) -> Body:
                return json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))) or b'null')

            def do_GET(self) -> None:
                prysm.requests.append(('GET', self.path))
                if self.path == '/api/v1/state':
                    self._reply(200, prysm.state())
                elif self.path == '/api/v1/colors/stream':
                    self._colours()
                elif self.path == '/api/v1/candidates/capture/device':
                    self._reply(200, [{'label': 'USB Video', 'fields': {'path': 'unique-id-1'}}])
                else:
                    self._reply(404, {'kind': 'not_found', 'message': self.path})

            def _colours(self) -> None:
                """Like Prysm: look every 100 ms, send on change; a comment each second finds a closed client."""
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.end_headers()
                sent: ColorsEvent | None = None
                ticks = 0
                try:
                    while prysm.running:
                        if prysm.target == 'processed':
                            event = prysm.colours(time.monotonic() - prysm.processed_at)
                        else:
                            event: ColorsEvent = {'type': 'idle'}
                        if event != sent:
                            self.wfile.write(b'data: ' + json.dumps(event).encode() + b'\n\n')
                            sent = event
                        elif ticks % 10 == 0:
                            self.wfile.write(b':\n\n')
                        self.wfile.flush()
                        ticks += 1
                        time.sleep(0.1)
                except OSError:
                    pass

            def do_PUT(self) -> None:
                body = self._body()
                prysm.requests.append(('PUT', self.path, body))
                if self.path == '/api/v1/target':
                    prysm.set_target(body['target'])
                    self._reply(204)
                elif self.path == '/api/v1/settings':
                    if body['expected'] != prysm.revision:
                        self._reply(409, {'kind': 'stale_revision', 'message': 'stale', 'current': prysm.revision})
                        return
                    prysm.settings = body['settings']
                    prysm.revision += 1
                    self._reply(200, {'revision': prysm.revision})
                else:
                    self._reply(404, {'kind': 'not_found', 'message': self.path})

            def do_POST(self) -> None:
                body = self._body()
                prysm.requests.append(('POST', self.path, body))
                if self.path == '/api/v1/capture/modes':

                    def span(lo: int, hi: int, step: int = 1) -> Span:
                        return {'min': lo, 'max': hi, 'step': step}

                    self._reply(
                        200,
                        {
                            'status': 'known',
                            'modes': [
                                {'width': span(1280, 1280), 'height': span(720, 720), 'fps': span(30, 60, 30)},
                                {'width': span(640, 640), 'height': span(480, 480), 'fps': span(30, 30)},
                            ],
                        },
                    )
                else:
                    self._reply(404, {'kind': 'not_found', 'message': self.path})

        return Handler
