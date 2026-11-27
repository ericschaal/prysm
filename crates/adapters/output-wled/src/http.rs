//! One blocking `POST /json/state` (or, for discovery, `GET /json/info`) per
//! call, done by a [`Deadline`] or a cut of its [`Hangup`], whichever comes
//! first: no retries, no keep-alive, no proxy. [`get`] and [`read_response`]
//! also serve the state checks, which never block.
use prysm_ports::{CallError, Hangup, Stopping};
use serde_json::Value;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Bounds name resolution with a mode change (its one exchange: connect,
/// request and response) however slowly the device answers. See
/// [`IDLE_LIMIT`](crate::output::IDLE_LIMIT) and
/// [`OPEN_LIMIT`](crate::output::OPEN_LIMIT), and what a stop waits for
/// beside them.
pub const TIMEOUT: Duration = Duration::from_millis(1500);

/// When the exchanges of one step must be done, and the budget it was
/// given, which a timeout names.
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    at: Instant,
    budget: Duration,
}

impl Deadline {
    #[must_use]
    pub fn after(budget: Duration) -> Self {
        Self {
            at: Instant::now() + budget,
            budget,
        }
    }

    /// This deadline, `by` sooner: the part of its budget before a last
    /// step that gets `by`.
    #[must_use]
    pub fn earlier(self, by: Duration) -> Self {
        Self {
            at: self.at.checked_sub(by).unwrap_or(self.at),
            budget: self.budget.saturating_sub(by),
        }
    }

    /// The time left, zero once it passed.
    #[must_use]
    pub fn remaining(self) -> Duration {
        self.at.saturating_duration_since(Instant::now())
    }

    /// The time left; [`RequestError::TimedOut`] once there is none.
    fn left(self) -> Result<Duration, RequestError> {
        let left = self.at.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(RequestError::TimedOut(self.budget));
        }
        Ok(left)
    }
}

/// WLED's response limit, including headers, framing and body bytes.
const MAX_RESPONSE: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    /// Nothing reached the device.
    #[error("cannot connect")]
    Connect(#[source] CallError),
    #[error("request failed")]
    Io(#[from] io::Error),
    #[error("malformed HTTP response: {0}")]
    Malformed(Malformed),
    /// The response exceeded the byte limit.
    #[error("response exceeds {} KiB", MAX_RESPONSE / 1024)]
    TooLarge,
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
    /// The [`Hangup`] was cut after the connection was made: the request
    /// may have reached the device.
    #[error("request not completed")]
    Stopped(#[from] Stopping),
}

/// What is wrong with a response that is not HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Malformed {
    /// The connection closed before the response was complete.
    #[error("connection closed mid-response")]
    Closed,
    #[error("header is not UTF-8")]
    HeaderNotUtf8,
    #[error("invalid status line")]
    StatusLine,
    #[error("invalid Content-Length")]
    ContentLength,
    #[error("invalid chunk size")]
    ChunkSize,
}

/// Send `state` to `device` by `deadline`, or until `stop` is cut, naming
/// `host` (the configured `host[:port]`) in the `Host` header, and require
/// WLED's `{"success":true}` reply.
pub fn post_state(
    device: SocketAddr,
    host: &str,
    state: &Value,
    deadline: Deadline,
    stop: &Hangup,
) -> Result<(), RequestError> {
    let body = state.to_string();
    let request = format!(
        "POST /json/state HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let body = exchange(device, &request, deadline, stop)?;
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
    let body = get_body(
        device,
        &device.to_string(),
        path,
        Deadline::after(timeout),
        &Hangup::default(),
    )?;
    Ok(serde_json::from_slice(&body)?)
}

/// Read an addressed JSON endpoint within the caller's deadline and cut.
pub(crate) fn get_body(
    device: SocketAddr,
    host: &str,
    path: &str,
    deadline: Deadline,
    stop: &Hangup,
) -> Result<Vec<u8>, RequestError> {
    exchange(device, &get(path, host), deadline, stop)
}

/// The request text of `GET path` from `host`, asking for JSON.
pub fn get(path: &str, host: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n\
         Connection: close\r\n\r\n"
    )
}

/// Connect to `device`, send `request` and read the whole response, all by
/// `deadline`: a peer that answers a byte at a time cannot extend it. A cut
/// of `stop` ends it at once: a connect not made yet fails as
/// [`RequestError::Connect`] (with [`CallError::Stopped`]), anything later as
/// [`RequestError::Stopped`].
fn exchange(
    device: SocketAddr,
    request: &str,
    deadline: Deadline,
    stop: &Hangup,
) -> Result<Vec<u8>, RequestError> {
    deadline.left()?;
    let (stream, _cut) = stop
        .connect(device, deadline.at)
        .map_err(RequestError::Connect)?;
    let mut stream = Bounded { stream, deadline };
    let response = stream
        .write_all(request.as_bytes())
        .map_err(RequestError::from)
        .and_then(|()| read_response(&mut stream, &mut Vec::new()));
    // A read the cut ended returns no more data, which can pass for the
    // end of a response.
    if stop.is_cut() {
        return Err(Stopping.into());
    }
    let timed_out = RequestError::TimedOut(deadline.budget);
    match response {
        Ok(Some(response)) => Ok(response),
        // A blocking read that would block has timed out.
        Ok(None) => Err(timed_out),
        Err(RequestError::Io(error)) if expired(&error) => Err(timed_out),
        Err(error) => Err(error),
    }
}

/// A stream whose every read and write waits only for what is left until
/// `deadline`.
struct Bounded {
    stream: TcpStream,
    deadline: Deadline,
}

impl Bounded {
    /// The time left; a timed-out error once there is none (a zero socket
    /// timeout would mean "wait forever").
    fn left(&self) -> io::Result<Option<Duration>> {
        self.deadline
            .left()
            .map(Some)
            .map_err(|_| ErrorKind::TimedOut.into())
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
    matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
}

/// Reads what `stream` has into `data`, which holds what earlier calls read:
/// the body once complete, `None` when a read would block first (more
/// may come). A complete non-2xx response or more than [`MAX_RESPONSE`] bytes fail.
pub fn read_response(
    stream: &mut impl Read,
    data: &mut Vec<u8>,
) -> Result<Option<Vec<u8>>, RequestError> {
    let mut chunk = [0; 4096];
    loop {
        let count = match stream.read(&mut chunk) {
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if data.len() + count > MAX_RESPONSE {
            return Err(RequestError::TooLarge);
        }
        data.extend_from_slice(&chunk[..count]);
        if let Some(response) = parse(data, count == 0)? {
            return Ok(Some(response));
        }
        if count == 0 {
            return Err(RequestError::Malformed(Malformed::Closed));
        }
    }
}

/// The response in `data`, or `None` while it is incomplete; `eof`: no more
/// data follows.
fn parse(data: &[u8], eof: bool) -> Result<Option<Vec<u8>>, RequestError> {
    let Some(end) = find(data, b"\r\n\r\n") else {
        return Ok(None);
    };
    let head = std::str::from_utf8(&data[..end])
        .map_err(|_| RequestError::Malformed(Malformed::HeaderNotUtf8))?;
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
        .ok_or(RequestError::Malformed(Malformed::StatusLine))?;
    let header = |name: &str| {
        lines.clone().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    };
    let body = &data[end + 4..];
    let complete =
        if header("transfer-encoding").is_some_and(|value| value.eq_ignore_ascii_case("chunked")) {
            dechunk(body)?
        } else if let Some(length) = header("content-length") {
            let length: usize = length
                .parse()
                .map_err(|_| RequestError::Malformed(Malformed::ContentLength))?;
            body.get(..length).map(<[u8]>::to_vec)
        } else {
            eof.then(|| body.to_vec())
        };
    if complete.is_some() && !(200..300).contains(&status) {
        return Err(RequestError::Status(status));
    }
    Ok(complete)
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
            .ok_or(RequestError::Malformed(Malformed::ChunkSize))?;
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

    /// A deadline split for a last step: the first part ends that much
    /// sooner and names what is left of the budget; the whole keeps both.
    #[test]
    fn an_earlier_deadline_ends_sooner_by_its_last_step() {
        let start = Instant::now();
        let whole = Deadline {
            at: start + Duration::from_secs(3),
            budget: Duration::from_secs(3),
        };
        let first = whole.earlier(Duration::from_secs(1));
        assert_eq!(first.at, start + Duration::from_secs(2));
        assert_eq!(first.budget, Duration::from_secs(2));
        assert_eq!(whole.at, start + Duration::from_secs(3));
        assert!(first.remaining() <= Duration::from_secs(2));
    }

    /// A passed deadline has nothing left, and says so with its budget: so
    /// a second request under the deadline of a first that used it up is
    /// not sent.
    #[test]
    fn a_passed_deadline_has_nothing_left() {
        let passed = Deadline {
            at: Instant::now(),
            budget: Duration::from_secs(3),
        };
        assert_eq!(passed.remaining(), Duration::ZERO);
        assert!(matches!(
            passed.left(),
            Err(RequestError::TimedOut(budget)) if budget == Duration::from_secs(3)
        ));
        let split = passed.earlier(Duration::from_secs(5));
        assert_eq!(split.remaining(), Duration::ZERO);
        assert_eq!(split.budget, Duration::ZERO);
    }

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

    /// The exchange ends with the timeout of its whole deadline: a bound
    /// per read would let a dripping answer go on, and end otherwise.
    fn assert_times_out(device: SocketAddr) {
        let error = get_json(device, "/json/info", LIMIT).unwrap_err();
        assert!(matches!(error, RequestError::TimedOut(LIMIT)), "{error:?}");
        assert_eq!(error.to_string(), "no complete response within 200ms");
    }

    #[test]
    fn a_status_error_requires_a_complete_response() {
        let mut response = b"HTTP/1.1 404 Missing\r\nContent-Length: 1\r\n\r\n".to_vec();
        assert_eq!(parse(&response, false).unwrap(), None);
        let error = read_response(&mut response.as_slice(), &mut Vec::new()).unwrap_err();
        assert!(matches!(error, RequestError::Malformed(Malformed::Closed)));
        response.push(b'x');
        assert!(matches!(
            parse(&response, false),
            Err(RequestError::Status(404))
        ));
    }

    #[test]
    fn the_limit_counts_headers_and_bytes_received_before_would_block() {
        struct WouldBlock;
        impl Read for WouldBlock {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(ErrorKind::WouldBlock.into())
            }
        }

        let head = b"HTTP/1.1 200 OK\r\n\r\n";
        let mut response = head.to_vec();
        response.resize(64 * 1024, b'x');
        let mut pending = Vec::new();
        let mut stream = response.as_slice().chain(WouldBlock);
        assert_eq!(read_response(&mut stream, &mut pending).unwrap(), None);
        let body = read_response(&mut io::empty(), &mut pending.clone()).unwrap();
        assert_eq!(body.as_deref(), Some(&response[head.len()..]));

        let error = read_response(&mut &b"x"[..], &mut pending).unwrap_err();
        assert!(matches!(error, RequestError::TooLarge));
        assert_eq!(error.to_string(), "response exceeds 64 KiB");
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
