//! One blocking `POST /json/state` (or, for discovery, `GET /json/info`) per
//! call, done by a [`Deadline`]: no retries, no keep-alive, no proxy. [`get`]
//! and [`read_response`] also serve the state checks, which never block.
use serde_json::Value;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Bounds name resolution with a mode change (its exchanges together:
/// connect, request and response) however slowly the device answers. See
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
/// The most a response may hold, head included: `/json/info` is about
/// 1.5 KiB, more with usermods.
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
    /// The response held more than this many bytes.
    #[error("response exceeds {} KiB", .0 / 1024)]
    TooLarge(usize),
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

/// Send `state` to `device` by `deadline`, naming `host` (the configured
/// `host[:port]`) in the `Host` header, and require WLED's
/// `{"success":true}` reply.
pub fn post_state(
    device: SocketAddr,
    host: &str,
    state: &Value,
    deadline: Deadline,
) -> Result<(), RequestError> {
    let body = state.to_string();
    let request = format!(
        "POST /json/state HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let body = exchange(device, &request, deadline)?.success()?;
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
    let body = exchange(
        device,
        &get(path, &device.to_string()),
        Deadline::after(timeout),
    )?
    .success()?;
    Ok(serde_json::from_slice(&body)?)
}

/// The request text of `GET path` from `host`, asking for JSON.
pub fn get(path: &str, host: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n\
         Connection: close\r\n\r\n"
    )
}

/// Connect to `device`, send `request` and read the whole response, all by
/// `deadline`: a peer that answers a byte at a time cannot extend it.
fn exchange(
    device: SocketAddr,
    request: &str,
    deadline: Deadline,
) -> Result<Response, RequestError> {
    let stream =
        TcpStream::connect_timeout(&device, deadline.left()?).map_err(RequestError::Connect)?;
    let mut stream = Bounded { stream, deadline };
    let response = stream
        .write_all(request.as_bytes())
        .map_err(RequestError::from)
        .and_then(|()| read_response(&mut stream, &mut Vec::new(), MAX_RESPONSE));
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
/// the response once complete, `None` when a read would block first (more
/// may come). More than `limit` bytes fail.
pub fn read_response(
    stream: &mut impl Read,
    data: &mut Vec<u8>,
    limit: usize,
) -> Result<Option<Response>, RequestError> {
    let mut chunk = [0; 4096];
    loop {
        let count = match stream.read(&mut chunk) {
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        data.extend_from_slice(&chunk[..count]);
        if data.len() > limit {
            return Err(RequestError::TooLarge(limit));
        }
        if let Some(response) = parse(data, count == 0)? {
            return Ok(Some(response));
        }
        if count == 0 {
            return Err(RequestError::Malformed("connection closed mid-response"));
        }
    }
}

/// The response in `data`, or `None` while it is incomplete; `eof`: no more
/// data follows.
fn parse(data: &[u8], eof: bool) -> Result<Option<Response>, RequestError> {
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
    fn a_response_over_the_limit_fails_naming_it() {
        let response = format!("HTTP/1.1 200 OK\r\n\r\n{}", "x".repeat(MAX_RESPONSE));
        let error =
            read_response(&mut response.as_bytes(), &mut Vec::new(), MAX_RESPONSE).unwrap_err();
        assert!(
            matches!(error, RequestError::TooLarge(MAX_RESPONSE)),
            "{error:?}"
        );
        assert_eq!(error.to_string(), "response exceeds 20 KiB");
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
