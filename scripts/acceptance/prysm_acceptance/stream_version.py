"""The capture stream's version handshake, from this computer's side.

`ask` sends a WebSocket upgrade offering one subprotocol to a Prysm and
returns its status and `Sec-WebSocket-Protocol`. `OtherVersion` is a
stand-in Prysm that answers every upgrade with 426 naming another version,
so a TV's slim worker shows its version failure.
"""

import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
import socket
import threading
from typing import cast
from urllib.parse import urlsplit

CURRENT = 'prysm-capture.v5'
OTHER = 'prysm-capture.v4'
PATH = '/api/v1/capture/stream'


def ask(base: str, protocol: str = OTHER, timeout: float = 5.0) -> tuple[int, str | None]:
    parts = urlsplit(base)
    key = base64.b64encode(os.urandom(16)).decode()
    request = (
        f'GET {PATH} HTTP/1.1\r\nHost: {parts.netloc}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
        f'Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n'
        f'Sec-WebSocket-Protocol: {protocol}\r\n\r\n'
    )
    # A URL without a host is passed on as it is, and fails in the connection.
    address = (cast('str', parts.hostname), parts.port or 80)
    with socket.create_connection(address, timeout=timeout) as connection:
        connection.sendall(request.encode())
        head = b''
        while b'\r\n\r\n' not in head and len(head) < 16384:
            chunk = connection.recv(4096)
            if not chunk:
                break
            head += chunk
    lines = head.split(b'\r\n\r\n')[0].decode(errors='replace').split('\r\n')
    status = int(lines[0].split()[1])
    headers = {k.strip().lower(): v.strip() for k, _, v in (line.partition(':') for line in lines[1:])}
    return status, headers.get('sec-websocket-protocol')


class OtherVersion:
    def __init__(self, port: int, host: str = '0.0.0.0') -> None:
        self.requests = 0
        self.server = ThreadingHTTPServer((host, port), self._handler())
        self.server.daemon_threads = True

    @property
    def port(self) -> int:
        return self.server.server_address[1]

    def start(self) -> 'OtherVersion':
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        return self

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()

    def _handler(self) -> type[BaseHTTPRequestHandler]:
        stand_in = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: object) -> None:
                pass

            def do_GET(self) -> None:
                stand_in.requests += 1
                body = b'{"kind":"bad_request","message":"another capture stream version"}'
                self.send_response(426)
                self.send_header('Sec-WebSocket-Protocol', OTHER)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                self.send_header('Connection', 'close')
                self.end_headers()
                self.wfile.write(body)

        return Handler
