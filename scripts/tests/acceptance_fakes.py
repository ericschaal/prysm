"""A stand-in Prysm portal for the acceptance runner's tests.

It answers the API calls the desktop checks make and, like Prysm, drives a
`FakeWled` (in-process): live mode and DDP while processed, `lor: 2` for
device effects, off for off, and reports WLED's brightness while it scales
the colours. `send_ddp = False` makes it forget DDP, as a broken output would.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
import time

COLOURS = {'type': 'colors', 'top': [[250, 10, 10]] * 8, 'right': [[10, 250, 10]] * 8,
           'bottom': [[10, 10, 250]] * 8, 'left': [[250, 250, 10]] * 8}


class FakePrysm:
    def __init__(self, wled, settings):
        self.wled = wled
        self.target = 'off'
        self.settings = settings
        self.revision = 41
        self.send_ddp = True
        self.requests = []
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), self._handler())
        self.server.daemon_threads = True
        self.running = True

    @property
    def url(self):
        return f'http://127.0.0.1:{self.server.server_address[1]}'

    def start(self):
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        threading.Thread(target=self._ddp, daemon=True).start()
        return self

    def close(self):
        self.running = False
        self.server.shutdown()
        self.server.server_close()

    def _ddp(self):
        while self.running:
            if self.target == 'processed' and self.send_ddp:
                with self.wled.lock:
                    self.wled.ddp_at = time.monotonic()
                    self.wled.ddp_from = '127.0.0.1'
            time.sleep(0.1)

    def state(self):
        state = {'target': self.target, 'settings': self.settings, 'revision': self.revision, 'failure': None,
                 'surface_failures': [],
                 'session': {'state': {'off': 'stopped', 'processed': 'running', 'device_native': 'native'}[self.target]}}
        if self.target == 'processed':
            state['input'] = {'fps': 30.0, 'receiving': True, 'sent_fps': 30.0, 'latency': {'output_ms': 1}}
            with self.wled.lock:
                force_max = self.wled.cfg['if']['live']['maxbri']
                bri = self.wled.state['bri']
            if not force_max:
                state['output_brightness'] = {'percent': round(bri * 100 / 255)}
        return state

    def set_target(self, target):
        self.target = target
        body = {'processed': {'on': True, 'lor': 0, 'live': True, 'tt': 0},
                'device_native': {'on': True, 'lor': 2, 'tt': 0}, 'off': {'on': False, 'lor': 2, 'tt': 0}}[target]
        if target != 'processed':
            self.wled.post_state({'live': False})
        self.wled.post_state(body)

    def _handler(prysm):
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def _reply(self, status, body=None):
                data = b'' if body is None else json.dumps(body).encode()
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def _body(self):
                return json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))) or b'null')

            def do_GET(self):
                prysm.requests.append(('GET', self.path))
                if self.path == '/api/v1/state':
                    self._reply(200, prysm.state())
                elif self.path == '/api/v1/colors/stream':
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.end_headers()
                    event = COLOURS if prysm.target == 'processed' else {'type': 'idle'}
                    self.wfile.write(b'data: ' + json.dumps(event).encode() + b'\n\n')
                    self.wfile.flush()
                    time.sleep(0.3)
                elif self.path == '/api/v1/candidates/capture/device':
                    self._reply(200, [{'label': 'USB Video', 'fields': {'path': 'unique-id-1'}}])
                else:
                    self._reply(404, {'kind': 'not_found', 'message': self.path})

            def do_PUT(self):
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

            def do_POST(self):
                body = self._body()
                prysm.requests.append(('POST', self.path, body))
                if self.path == '/api/v1/capture/modes':
                    span = lambda lo, hi, step=1: {'min': lo, 'max': hi, 'step': step}  # noqa: E731
                    self._reply(200, {'status': 'known', 'modes': [
                        {'width': span(1280, 1280), 'height': span(720, 720), 'fps': span(30, 60, 30)},
                        {'width': span(640, 640), 'height': span(480, 480), 'fps': span(30, 30)}]})
                else:
                    self._reply(404, {'kind': 'not_found', 'message': self.path})
        return Handler
