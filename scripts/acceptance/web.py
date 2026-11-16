"""A small JSON-over-HTTP client with timeouts, and a server-sent events reader."""
from dataclasses import dataclass
import errno
import http.client
import json
import socket
import time
from urllib.parse import urlsplit

LOCAL_NETWORK_HINT = (
    'macOS refused the connection (EHOSTUNREACH): allow Local Network access for the app '
    'that runs this script (System Settings > Privacy & Security > Local Network), '
    'or run it from Terminal.')


@dataclass
class Reply:
    status: int
    headers: dict
    body: bytes

    def json(self):
        return json.loads(self.body) if self.body else None


class Unreachable(RuntimeError):
    pass


def _connection(url, timeout):
    parts = urlsplit(url)
    if parts.scheme not in ('http', 'https'):
        raise ValueError(f'not an http URL: {url}')
    kind = http.client.HTTPSConnection if parts.scheme == 'https' else http.client.HTTPConnection
    path = parts.path or '/'
    if parts.query:
        path += '?' + parts.query
    return kind(parts.hostname, parts.port, timeout=timeout), path


def request(method, url, body=None, timeout=5.0, headers=None):
    """One request; any HTTP status is a `Reply`, a network failure `Unreachable`."""
    connection, path = _connection(url, timeout)
    data = None
    sent = dict(headers or {})
    if body is not None:
        data = json.dumps(body).encode()
        sent['Content-Type'] = 'application/json'
    try:
        connection.request(method, path, body=data, headers=sent)
        response = connection.getresponse()
        return Reply(response.status, {k.lower(): v for k, v in response.getheaders()}, response.read())
    except OSError as error:
        hint = f' ({LOCAL_NETWORK_HINT})' if getattr(error, 'errno', None) == errno.EHOSTUNREACH else ''
        raise Unreachable(f'{method} {url}: {error}{hint}') from error
    finally:
        connection.close()


def get_json(url, timeout=5.0):
    reply = request('GET', url, timeout=timeout)
    if reply.status != 200:
        raise RuntimeError(f'GET {url}: HTTP {reply.status} {reply.body[:200]!r}')
    return reply.json()


def events(url, seconds, limit=None):
    """The `data:` payloads (parsed JSON) a server-sent events stream gives within `seconds`."""
    connection, path = _connection(url, min(seconds, 5.0) + 1)
    found = []
    deadline = time.monotonic() + seconds
    try:
        connection.request('GET', path, headers={'Accept': 'text/event-stream'})
        sock = connection.sock  # the response may take it over from the connection
        response = connection.getresponse()
        if response.status != 200:
            raise RuntimeError(f'GET {url}: HTTP {response.status}')
        while time.monotonic() < deadline and (limit is None or len(found) < limit):
            sock.settimeout(max(0.05, deadline - time.monotonic()))
            try:
                line = response.fp.readline()
            except (socket.timeout, TimeoutError):
                break
            if not line:
                break
            if line.startswith(b'data:'):
                found.append(json.loads(line[5:].strip()))
    except OSError as error:
        raise Unreachable(f'GET {url}: {error}') from error
    finally:
        connection.close()
    return found
