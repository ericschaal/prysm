"""A small JSON-over-HTTP client with timeouts, and a server-sent events reader."""

from dataclasses import dataclass
import errno
import http.client
import json
import time
from typing import Literal, TypeAlias, cast, overload
from urllib.parse import urlsplit

LOCAL_NETWORK_HINT = (
    'macOS refused the connection (EHOSTUNREACH): allow Local Network access for the app '
    'that runs this script (System Settings > Privacy & Security > Local Network), '
    'or run it from Terminal.'
)

# A JSON value as `json.loads` returns it. Callers that know its shape type it
# at their edge, as a TypedDict (`prysm_api`, `wled_api`, `tv`).
Json: TypeAlias = 'dict[str, Json] | list[Json] | str | int | float | bool | None'
JsonObject: TypeAlias = dict[str, Json]


@dataclass
class Reply:
    status: int
    headers: dict[str, str]
    body: bytes

    def json(self) -> Json:
        return json.loads(self.body) if self.body else None


class Unreachable(RuntimeError):
    pass


def _connection(url: str, timeout: float) -> tuple[http.client.HTTPConnection, str]:
    parts = urlsplit(url)
    if parts.scheme not in ('http', 'https'):
        raise ValueError(f'not an http URL: {url}')
    kind = http.client.HTTPSConnection if parts.scheme == 'https' else http.client.HTTPConnection
    path = parts.path or '/'
    if parts.query:
        path += '?' + parts.query
    # A URL without a host is passed on as it is, and fails in the connection.
    return kind(cast('str', parts.hostname), parts.port, timeout=timeout), path


def request(
    method: str, url: str, body: object = None, timeout: float = 5.0, headers: dict[str, str] | None = None
) -> Reply:
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
        hint = f' ({LOCAL_NETWORK_HINT})' if error.errno == errno.EHOSTUNREACH else ''
        raise Unreachable(f'{method} {url}: {error}{hint}') from error
    finally:
        connection.close()


def get_json(url: str, timeout: float = 5.0) -> Json:
    reply = request('GET', url, timeout=timeout)
    if reply.status != 200:
        raise RuntimeError(f'GET {url}: HTTP {reply.status} {reply.body[:200]!r}')
    return reply.json()


@overload
def events(url: str, seconds: float, limit: int | None = None, *, timed: Literal[False] = False) -> list[Json]: ...
@overload
def events(url: str, seconds: float, limit: int | None = None, *, timed: Literal[True]) -> list[tuple[float, Json]]: ...
def events(
    url: str, seconds: float, limit: int | None = None, *, timed: bool = False
) -> list[Json] | list[tuple[float, Json]]:
    """The `data:` payloads (parsed JSON) a server-sent events stream gives within `seconds`.

    `timed`: each as (seconds since the request, payload), by arrival. A stream that ends sooner gives what it
    gave; the reader on a device (`commands.COLOURS_READER`) fails then instead, on purpose."""
    connection, path = _connection(url, min(seconds, 5.0) + 1)
    found: list[tuple[float, Json]] = []
    start = time.monotonic()
    deadline = start + seconds
    try:
        connection.request('GET', path, headers={'Accept': 'text/event-stream'})
        sock = connection.sock  # the response may take it over from the connection
        response = connection.getresponse()
        if response.status != 200:
            raise RuntimeError(f'GET {url}: HTTP {response.status}')
        while time.monotonic() < deadline and (limit is None or len(found) < limit):
            sock.settimeout(max(0.05, deadline - time.monotonic()))
            try:
                line = response.readline()  # de-chunked: an event split across chunks is one line
            except TimeoutError:
                break
            if not line:
                break
            if line.startswith(b'data:'):
                found.append((round(time.monotonic() - start, 3), json.loads(line[5:].strip())))
    except OSError as error:
        raise Unreachable(f'GET {url}: {error}') from error
    finally:
        connection.close()
    return found if timed else [payload for _, payload in found]
