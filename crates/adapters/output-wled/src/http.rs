//! One blocking `POST /json/state` (or, for discovery, `GET /json/info`) per
//! call: no retries, no keep-alive, no proxy. [`parse`] also reads the state
//! checks' replies.
use serde_json::Value;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Bounds name resolution, and one whole exchange (connect, request and
/// response) however slowly the device answers. Idling sends two requests,
/// so it ends within 3 s, inside the runtime stop deadline (6 s on the TV).
pub const TIMEOUT: Duration = Duration::from_millis(1500);
/// `/json/info` is about 1.5 KiB, more with usermods.
const MAX_BODY: usize = 16 * 1024;
const MAX_RESPONSE: usize = 20 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    /// Nothing reached the device.
    #[error("cannot connect")]
    Connect(#[source] io::Error),
    #[error("request failed")]
    Io(#[from] io::Error),
    #[error("malformed HTTP response: {0}")]
    Malformed(&'static str),
    #[error("device answered HTTP {0}")]
    Status(u16),
    #[error("response is not JSON")]
    Json(#[from] serde_json::Error),
    #[error("device rejected the request: {0}")]
    Rejected(String),
    #[error("device did not acknowledge the request")]
    Unacknowledged,
    #[error("no complete response within {0:?}")]
    TimedOut(Duration),
}

/// A complete HTTP response.
#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    /// The body of a 2xx response.
    ///
    /// # Errors
    /// [`RequestError::Status`] for any other status.
    pub fn success(self) -> Result<Vec<u8>, RequestError> {
        if (200..300).contains(&self.status) {
            Ok(self.body)
        } else {
            Err(RequestError::Status(self.status))
        }
    }
}

/// Send `state` to `device`, naming `host` (the configured `host[:port]`) in
/// the `Host` header, and require WLED's `{"success":true}` reply.
pub fn post_state(device: SocketAddr, host: &str, state: &Value) -> Result<(), RequestError> {
    let body = state.to_string();
    let request = format!(
        "POST /json/state HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let body = exchange(device, &request, TIMEOUT)?.success()?;
    let reply: Value = serde_json::from_slice(&body)?;
    if let Some(error) = reply.get("error") {
        return Err(RequestError::Rejected(error.to_string()));
    }
    match reply.get("success").and_then(Value::as_bool) {
        Some(true) => Ok(()),
        Some(false) => Err(RequestError::Rejected("success is false".into())),
        None => Err(RequestError::Unacknowledged),
    }
}

/// `GET path` from `device` and its JSON reply, waiting at most `timeout`
/// for the whole exchange.
pub fn get_json(device: SocketAddr, path: &str, timeout: Duration) -> Result<Value, RequestError> {
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {device}\r\nAccept: application/json\r\n\
         Connection: close\r\n\r\n"
    );
    let body = exchange(device, &request, timeout)?.success()?;
    Ok(serde_json::from_slice(&body)?)
}

/// Connect to `device`, send `request` and read the whole response, all
/// within `limit`: a peer that answers a byte at a time cannot extend it.
fn exchange(device: SocketAddr, request: &str, limit: Duration) -> Result<Response, RequestError> {
    let deadline = Instant::now() + limit;
    let stream = TcpStream::connect_timeout(&device, limit).map_err(RequestError::Connect)?;
    let mut stream = Bounded { stream, deadline };
    let response = stream
        .write_all(request.as_bytes())
        .map_err(RequestError::from)
        .and_then(|()| read_response(&mut stream));
    match response {
        Err(RequestError::Io(error)) if expired(&error) => Err(RequestError::TimedOut(limit)),
        response => response,
    }
}

/// A stream whose every read and write waits only for what is left until
/// `deadline`.
struct Bounded {
    stream: TcpStream,
    deadline: Instant,
}

impl Bounded {
    /// The time left; a timed-out error once there is none (a zero socket
    /// timeout would mean "wait forever").
    fn left(&self) -> io::Result<Option<Duration>> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(Some(left))
    }
}

impl Read for Bounded {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(self.left()?)?;
        self.stream.read(buffer)
    }
}

impl Write for Bounded {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(self.left()?)?;
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// Whether `error` is a socket timeout: with [`Bounded`], the deadline passed.
fn expired(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

fn read_response(stream: &mut impl Read) -> Result<Response, RequestError> {
    let mut data = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        let count = stream.read(&mut chunk)?;
        data.extend_from_slice(&chunk[..count]);
        if let Some(response) = parse(&data, count == 0)? {
            if response.body.len() > MAX_BODY {
                return Err(RequestError::Malformed("body exceeds 16 KiB"));
            }
            return Ok(response);
        }
        if count == 0 {
            return Err(RequestError::Malformed("connection closed mid-response"));
        }
        if data.len() > MAX_RESPONSE {
            return Err(RequestError::Malformed("response exceeds 20 KiB"));
        }
    }
}

/// The response in `data`, or `None` while it is incomplete; `eof`: no more
/// data follows.
pub fn parse(data: &[u8], eof: bool) -> Result<Option<Response>, RequestError> {
    let Some(end) = find(data, b"\r\n\r\n") else {
        return Ok(None);
    };
    let head = std::str::from_utf8(&data[..end])
        .map_err(|_| RequestError::Malformed("header is not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| {
            let mut parts = line.split(' ');
            parts
                .next()
                .filter(|version| version.starts_with("HTTP/1."))?;
            parts.next()?.parse().ok()
        })
        .ok_or(RequestError::Malformed("invalid status line"))?;
    let header = |name: &str| {
        lines.clone().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    };
    let body = &data[end + 4..];
    if header("transfer-encoding").is_some_and(|value| value.eq_ignore_ascii_case("chunked")) {
        return Ok(dechunk(body)?.map(|body| Response { status, body }));
    }
    if let Some(length) = header("content-length") {
        let length: usize = length
            .parse()
            .map_err(|_| RequestError::Malformed("invalid Content-Length"))?;
        return Ok((body.len() >= length).then(|| Response {
            status,
            body: body[..length].to_vec(),
        }));
    }
    Ok(eof.then(|| Response {
        status,
        body: body.to_vec(),
    }))
}

fn dechunk(mut rest: &[u8]) -> Result<Option<Vec<u8>>, RequestError> {
    let mut body = Vec::new();
    loop {
        let Some(line) = find(rest, b"\r\n") else {
            return Ok(None);
        };
        let size = std::str::from_utf8(&rest[..line])
            .ok()
            .and_then(|text| usize::from_str_radix(text.split(';').next()?.trim(), 16).ok())
            .ok_or(RequestError::Malformed("invalid chunk size"))?;
        rest = &rest[line + 2..];
        if size == 0 {
            return Ok(Some(body));
        }
        if rest.len() < size.saturating_add(2) {
            return Ok(None);
        }
        body.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

fn find(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).position(|part| part == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    const LIMIT: Duration = Duration::from_millis(200);
    /// Far below [`LIMIT`], so each byte alone would reset a per-read timeout.
    const DRIP: Duration = Duration::from_millis(10);

    /// A device that accepts one connection, reads the request and hands the
    /// connection to `answer`.
    fn device(answer: impl FnOnce(TcpStream) + Send + 'static) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _ = socket.read(&mut [0; 1024]);
            answer(socket);
        });
        address
    }

    fn assert_times_out(device: SocketAddr) {
        let started = Instant::now();
        let error = get_json(device, "/json/info", LIMIT).unwrap_err();
        assert!(matches!(error, RequestError::TimedOut(LIMIT)), "{error:?}");
        assert!(started.elapsed() < LIMIT * 3, "{:?}", started.elapsed());
    }

    #[test]
    fn a_device_that_never_answers_times_out() {
        assert_times_out(device(|mut socket| {
            // Holds the connection open until the client gives up.
            let _ = socket.read(&mut [0; 1]);
        }));
    }

    #[test]
    fn a_device_that_drips_its_answer_cannot_extend_the_exchange() {
        assert_times_out(device(|mut socket| {
            let head = b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n";
            for byte in head.iter().chain(std::iter::repeat(&b' ')) {
                if socket.write_all(&[*byte]).is_err() {
                    return;
                }
                thread::sleep(DRIP);
            }
        }));
    }
}
