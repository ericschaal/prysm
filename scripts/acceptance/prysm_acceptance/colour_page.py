"""A full-screen colour page the runner controls, for the display check.

The page (served on localhost only) asks every 100 ms which field to show
and reports back what it shows, so the runner knows when a field is on
screen. The operator opens it once on the display the capture card
records (this Mac's HDMI output) and makes it full screen.
"""

from collections.abc import Sequence
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading
import time
from typing import TypeAlias, TypedDict

from prysm_acceptance.prysm_api import ColorsEvent, Edge

Rgb: TypeAlias = tuple[int, int, int]
# Each edge's colour in a field.
Edges: TypeAlias = dict[Edge, Rgb]


class Field(TypedDict):
    """What the page shows (`/field`): a field's name and edges; no name and no edges: the waiting page."""

    name: str
    edges: Edges | None


class EdgeMatch(TypedDict):
    mean: tuple[int, ...]
    seen: str
    expected: str


# Fields: a name and the colour of each edge (top, right, bottom, left) as RGB.
FIELDS: list[tuple[str, Edges]] = [
    ('red', {'top': (255, 0, 0), 'right': (255, 0, 0), 'bottom': (255, 0, 0), 'left': (255, 0, 0)}),
    ('green', {'top': (0, 255, 0), 'right': (0, 255, 0), 'bottom': (0, 255, 0), 'left': (0, 255, 0)}),
    ('blue', {'top': (0, 0, 255), 'right': (0, 0, 255), 'bottom': (0, 0, 255), 'left': (0, 0, 255)}),
    ('white', {'top': (255, 255, 255), 'right': (255, 255, 255), 'bottom': (255, 255, 255), 'left': (255, 255, 255)}),
    ('edges', {'top': (255, 0, 0), 'right': (0, 255, 0), 'bottom': (0, 0, 255), 'left': (255, 255, 0)}),
]

PAGE = """<!doctype html><html><head><meta charset="utf-8"><title>Prysm colour fields</title>
<style>html,body{margin:0;height:100%;background:#000;cursor:none;overflow:hidden}
div{position:absolute}#c{inset:0}#t{left:20%;right:20%;top:0;height:20%}#b{left:20%;right:20%;bottom:0;height:20%}
#l{left:0;top:20%;bottom:20%;width:20%}#r{right:0;top:20%;bottom:20%;width:20%}
#m{left:20%;right:20%;top:20%;bottom:20%;background:#404040}
#h{left:2em;top:2em;color:#888;font:24px sans-serif}</style></head><body>
<div id="c"></div><div id="t"></div><div id="b"></div><div id="l"></div><div id="r"></div><div id="m"></div>
<div id="h">Move this window to the capture display and press F (full screen). Waiting for the runner...</div>
<script>
const rgb = c => `rgb(${c[0]},${c[1]},${c[2]})`;
let shown = '';
document.addEventListener('keydown', e => { if (e.key === 'f') document.documentElement.requestFullscreen(); });
async function tick() {
  try {
    const field = await (await fetch('/field')).json();
    if (field.name !== shown) {
      const e = field.edges;
      document.getElementById('h').style.display = field.name ? 'none' : 'block';
      for (const [id, edge] of [['t','top'],['b','bottom'],['l','left'],['r','right']])
        document.getElementById(id).style.background = e ? rgb(e[edge]) : '#000';
      document.getElementById('m').style.display = field.name === 'edges' ? 'block' : 'none';
      document.getElementById('c').style.background = !e ? '#000' : field.name === 'edges' ? '#404040' : rgb(e.top);
      shown = field.name;
      await fetch('/shown', {method: 'POST', body: JSON.stringify({name: shown, full: !!document.fullscreenElement})});
    }
  } catch (e) {}
  setTimeout(tick, 100);
}
tick();
</script></body></html>"""


class Shown(TypedDict, total=False):
    """What the page reports it shows (`POST /shown`), and whether it is full screen."""

    name: str
    full: bool


class ColourPage:
    def __init__(self, port: int) -> None:
        self.field: Field = {'name': '', 'edges': None}
        self.shown: Shown | None = None
        self.changed = threading.Condition()
        self.server = ThreadingHTTPServer(('127.0.0.1', port), self._handler())
        self.server.daemon_threads = True

    @property
    def url(self) -> str:
        return f'http://127.0.0.1:{self.server.server_address[1]}/'

    def start(self) -> 'ColourPage':
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        return self

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()

    def show(self, name: str, edges: Edges | None, seconds: float = 10.0) -> bool:
        """Show a field; True once the page reports it on screen."""
        with self.changed:
            self.field = {'name': name, 'edges': edges}
            deadline = time.monotonic() + seconds
            while not (self.shown and self.shown.get('name') == name):
                left = deadline - time.monotonic()
                if left <= 0:
                    return False
                self.changed.wait(left)
            return True

    def _handler(self) -> type[BaseHTTPRequestHandler]:
        page = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: object) -> None:
                pass

            def _send(self, data: bytes, kind: str) -> None:
                self.send_response(200)
                self.send_header('Content-Type', kind)
                self.send_header('Cache-Control', 'no-store')
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self) -> None:
                if self.path == '/field':
                    self._send(json.dumps(page.field).encode(), 'application/json')
                else:
                    self._send(PAGE.encode(), 'text/html; charset=utf-8')

            def do_POST(self) -> None:
                body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                with page.changed:
                    page.shown = json.loads(body or b'{}')
                    page.changed.notify_all()
                self._send(b'{}', 'application/json')

        return Handler


# The colours an edge is told apart from: the fields' own and their neighbours.
PALETTE: dict[str, Rgb] = {
    'red': (255, 0, 0),
    'green': (0, 255, 0),
    'blue': (0, 0, 255),
    'yellow': (255, 255, 0),
    'cyan': (0, 255, 255),
    'magenta': (255, 0, 255),
    'white': (255, 255, 255),
    'gray': (64, 64, 64),
    'black': (0, 0, 0),
}


def nearest(colour: Sequence[int]) -> str:
    return min(PALETTE, key=lambda name: sum((a - b) ** 2 for a, b in zip(colour, PALETTE[name], strict=False)))


def matches(measured: ColorsEvent, expected: Edges) -> tuple[bool, dict[Edge, EdgeMatch | None]]:
    """Whether each edge's mean colour is nearest to the expected one in `PALETTE`.

    Capture, encoding and processing shift colours, so the check names the
    colour rather than comparing values. `measured` is a ColorsEvent (edges
    as lists of [r, g, b]); returns (ok, {edge: {mean, seen, expected}})."""
    found: dict[Edge, EdgeMatch | None] = {}
    ok = True
    for edge, colour in expected.items():
        samples: list[list[int]] = measured.get(edge) or []
        # The middle half: the corners blend two edges.
        if len(samples) >= 4:
            samples = samples[len(samples) // 4 : len(samples) - len(samples) // 4]
        if not samples:
            found[edge] = None
            ok = False
            continue
        mean = tuple(round(sum(s[i] for s in samples) / len(samples)) for i in range(3))
        match = found[edge] = {'mean': mean, 'seen': nearest(mean), 'expected': nearest(colour)}
        ok = ok and match['seen'] == match['expected']
    return ok, found
