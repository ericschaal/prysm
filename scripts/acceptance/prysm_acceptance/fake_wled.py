"""A stand-in WLED for rehearsing the desktop stage and for tests.

Answers the parts of WLED's JSON API Prysm and the runner use (`/json/si`,
`/json/state`, `/json/info`, `/json/cfg`, POST `/json/state` and
`/json/cfg`) and, with `--ddp`, receives DDP: like WLED it shows realtime
data while packets arrive (unless the live override `lor` is set) and drops
back 2.5 s after the last one. Prysm always sends DDP to UDP port 4048, so
a rehearsal needs `--ddp 127.0.0.1:4048`.

  uv run --project scripts/acceptance fake-wled --http 127.0.0.1:18080 --ddp 127.0.0.1:4048 --leds 162
"""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import socket
import threading
import time

from prysm_acceptance.web import Json, JsonObject

Address = tuple[str, int]

REALTIME_TIMEOUT = 2.5


def _merge(into: JsonObject, change: JsonObject) -> None:
    for key, value in change.items():
        inner = into.get(key)
        if isinstance(value, dict) and isinstance(inner, dict):
            _merge(inner, value)
        else:
            into[key] = value


class FakeWled:
    def __init__(self, leds: int = 162, http: Address = ('127.0.0.1', 0), ddp: Address | None = None) -> None:
        self.lock = threading.Lock()
        self.state: JsonObject = {'on': True, 'bri': 128, 'lor': 0, 'transition': 7, 'ps': -1}
        self.cfg: JsonObject = {
            'if': {'live': {'en': True, 'maxbri': False, 'no-gc': False}},
            'light': {'scale-bri': 100, 'gc': {'bri': 1, 'col': 2.2, 'val': 2.2}},
        }
        self.leds = leds
        self.json_live = False
        self.ddp_at = 0.0
        self.ddp_from = ''
        self.ddp_frames: list[float] = []  # arrival times of DDP frames (push flag)
        self.posts: list[JsonObject] = []
        self.server = ThreadingHTTPServer(http, self._handler())
        self.server.daemon_threads = True
        self.udp: socket.socket | None = None
        if ddp:
            self.udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            self.udp.bind(ddp)
            self.udp.settimeout(0.5)
        self.running = True

    @property
    def http_address(self) -> str:
        host, port = self.server.server_address[:2]
        return f'{host}:{port}'

    def start(self) -> 'FakeWled':
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        if self.udp:
            threading.Thread(target=self._receive, daemon=True).start()
        return self

    def close(self) -> None:
        self.running = False
        self.server.shutdown()
        self.server.server_close()
        if self.udp:
            self.udp.close()

    def _receive(self) -> None:
        udp = self.udp
        if udp is None:  # started only with --ddp
            return
        while self.running:
            try:
                packet, sender = udp.recvfrom(65536)
            except (TimeoutError, OSError):
                continue
            with self.lock:
                if self.state['lor'] == 0 and len(packet) >= 10:
                    self.ddp_at = time.monotonic()
                    self.ddp_from = sender[0]
                    if packet[0] & 0x01:  # push: the last packet of a frame
                        self.ddp_frames.append(self.ddp_at)
                        del self.ddp_frames[:-600]

    def ddp_fps(self, window: float = 3.0) -> float:
        now = time.monotonic()
        with self.lock:
            return sum(1 for t in self.ddp_frames if now - t <= window) / window

    def si(self) -> JsonObject:
        with self.lock:
            ddp = time.monotonic() - self.ddp_at < REALTIME_TIMEOUT
            live = ddp or self.json_live
            info: JsonObject = {
                'ver': '16.0.1-fake',
                'name': 'Fake WLED',
                'leds': {'count': self.leds},
                'live': live,
                'lm': 'DDP' if ddp else '',
                'lip': self.ddp_from if ddp else '',
            }
            return {'state': dict(self.state), 'info': info}

    def post_state(self, body: JsonObject) -> None:
        with self.lock:
            self.posts.append(body)
            if 'live' in body:
                self.json_live = bool(body['live'])
                if not body['live']:
                    self.ddp_at = 0.0
            for key in ('on', 'bri', 'lor', 'transition', 'ps'):
                if key in body:
                    self.state[key] = body[key]
            if 'tt' in body:
                pass  # a one-off transition

    def _handler(self) -> type[BaseHTTPRequestHandler]:
        wled = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: object) -> None:
                pass

            def _reply(self, body: Json, status: int = 200) -> None:
                data = json.dumps(body).encode()
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(data)))
                self.send_header('Connection', 'close')
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self) -> None:
                si = wled.si()
                pages = {'/json/si': si, '/json/state': si['state'], '/json/info': si['info'], '/json/cfg': wled.cfg}
                if self.path in pages:
                    self._reply(pages[self.path])
                else:
                    self._reply({'error': 'not found'}, 404)

            def do_POST(self) -> None:
                length = int(self.headers.get('Content-Length', 0))
                try:
                    body = json.loads(self.rfile.read(length) or b'{}')
                except ValueError:
                    self._reply({'error': 9}, 400)
                    return
                if self.path == '/json/state':
                    wled.post_state(body)
                elif self.path == '/json/cfg':
                    with wled.lock:
                        _merge(wled.cfg, body)
                else:
                    self._reply({'error': 'not found'}, 404)
                    return
                self._reply({'success': True})

        return Handler


def _pair(text: str) -> Address:
    host, port = text.rsplit(':', 1)
    return host, int(port)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--http', type=_pair, default=('127.0.0.1', 18080))
    parser.add_argument('--ddp', type=_pair, default=None)
    parser.add_argument('--leds', type=int, default=162)
    args = parser.parse_args()
    if args.http[0] not in ('127.0.0.1', 'localhost') or (args.ddp and args.ddp[0] not in ('127.0.0.1', 'localhost')):
        parser.error('the fake WLED listens on localhost only')
    wled = FakeWled(args.leds, args.http, args.ddp).start()
    print(
        f'fake WLED on http://{wled.http_address}' + (f', DDP on {args.ddp[0]}:{args.ddp[1]}' if args.ddp else ''),
        flush=True,
    )
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        wled.close()


if __name__ == '__main__':
    main()
