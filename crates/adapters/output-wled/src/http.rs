//! One blocking `POST /json/state` (or, for discovery, `GET /json/info`) per
//! call: no retries, no keep-alive, no proxy. [`parse`] also reads the state
//! checks' replies.
use serde_json::Value;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Bounds name resolution, connecting and each read or write, separately. Two requests at worst case
/// must fit inside the runtime stop deadline (10 s by default).
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
/// for the connect and for each read or write.
pub fn get_json(device: SocketAddr, path: &str, timeout: Duration) -> Result<Value, RequestError> {
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {device}\r\nAccept: application/json\r\n\
         Connection: close\r\n\r\n"
    );
    let body = exchange(device, &request, timeout)?.success()?;
    Ok(serde_json::from_slice(&body)?)
}

/// Connect to `device`, send `request` and read the whole response, waiting
/// at most `timeout` for the connect and for each read or write.
fn exchange(
    device: SocketAddr,
    request: &str,
    timeout: Duration,
) -> Result<Response, RequestError> {
    let mut stream = TcpStream::connect_timeout(&device, timeout).map_err(RequestError::Connect)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(request.as_bytes())?;
    read_response(&mut stream)
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
