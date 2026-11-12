//! One blocking `POST /json/state` per call: no retries, no keep-alive, no proxy.
//! [`parse`] also reads the state checks' replies.
use serde_json::Value;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Bounds name resolution, connecting and each read or write, separately. Two requests at worst case
/// must fit inside the runtime stop deadline (10 s by default).
pub const TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_BODY: usize = 4096;
const MAX_RESPONSE: usize = 8192;

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

/// Send `state` to `device`, naming `host` (the configured `host[:port]`) in
/// the `Host` header, and require WLED's `{"success":true}` reply.
pub fn post_state(device: SocketAddr, host: &str, state: &Value) -> Result<(), RequestError> {
    let mut stream = TcpStream::connect_timeout(&device, TIMEOUT).map_err(RequestError::Connect)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let body = state.to_string();
    let request = format!(
        "POST /json/state HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    let (status, body) = read_response(&mut stream)?;
    if !(200..300).contains(&status) {
        return Err(RequestError::Status(status));
    }
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

fn read_response(stream: &mut impl Read) -> Result<(u16, Vec<u8>), RequestError> {
    let mut data = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        let count = stream.read(&mut chunk)?;
        data.extend_from_slice(&chunk[..count]);
        if let Some((status, body)) = parse(&data, count == 0)? {
            if body.len() > MAX_BODY {
                return Err(RequestError::Malformed("body exceeds 4096 bytes"));
            }
            return Ok((status, body));
        }
        if count == 0 {
            return Err(RequestError::Malformed("connection closed mid-response"));
        }
        if data.len() > MAX_RESPONSE {
            return Err(RequestError::Malformed("response exceeds 8192 bytes"));
        }
    }
}

/// A response's status and body; `Ok(None)`: incomplete so far.
pub fn parse(data: &[u8], eof: bool) -> Result<Option<(u16, Vec<u8>)>, RequestError> {
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
        return Ok(dechunk(body)?.map(|body| (status, body)));
    }
    if let Some(length) = header("content-length") {
        let length: usize = length
            .parse()
            .map_err(|_| RequestError::Malformed("invalid Content-Length"))?;
        return Ok((body.len() >= length).then(|| (status, body[..length].to_vec())));
    }
    Ok(eof.then(|| (status, body.to_vec())))
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
