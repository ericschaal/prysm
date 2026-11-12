//! Whether WLED is showing Prysm's colours, from its own state.
//!
//! One `GET /json/si` at most every [`EVERY`] (5 × that after a check that got
//! no usable answer). The caller's thread only connects (bounded by
//! [`CONNECT`]) and sends the request; the reply is read without blocking on
//! later polls and abandoned after [`REPLY`]. Nothing is sent to change
//! WLED's state: Prysm reports, keeps sending, and the LEDs follow again once
//! WLED takes the colours.
use crate::http::{self, RequestError};
use prysm_ports::OutputIssue;
use serde::Deserialize;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// How often WLED's state is checked while it answers.
pub const EVERY: Duration = Duration::from_secs(1);
/// Longest the caller's thread waits, for the TCP connect (and the request
/// write, which fits an empty send buffer).
const CONNECT: Duration = Duration::from_millis(200);
/// How long a reply may take; it is read without waiting meanwhile.
const REPLY: Duration = Duration::from_secs(1);
/// `/json/si` grows with segments and usermods; WLED's own WebSocket limit.
const MAX_REPLY: usize = 64 * 1024;

pub struct Observer {
    device: SocketAddr,
    request: String,
    every: Duration,
    next: Instant,
    pending: Option<Pending>,
    /// The realtime source address WLED names for Prysm's colours: our own
    /// address, or behind NAT (Docker's bridge network) the first one WLED
    /// named while it showed DDP.
    ours: Option<IpAddr>,
    issue: Option<OutputIssue>,
}

struct Pending {
    stream: TcpStream,
    /// This host's address on the connection to WLED.
    local: IpAddr,
    reply: Vec<u8>,
    deadline: Instant,
}

impl Observer {
    /// The first check is `every` from now, once colours have reached WLED.
    pub fn new(device: SocketAddr, host: &str, every: Duration) -> Self {
        Self {
            device,
            request: format!(
                "GET /json/si HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n\
                 Connection: close\r\n\r\n"
            ),
            every,
            next: Instant::now() + every,
            pending: None,
            ours: None,
            issue: None,
        }
    }

    /// Advance the check without waiting for WLED; the latest finding.
    pub fn poll(&mut self) -> Option<OutputIssue> {
        let now = Instant::now();
        match self.pending.take() {
            Some(mut pending) => match pending.read() {
                Ok(None) if now < pending.deadline => self.pending = Some(pending),
                Ok(None) | Err(_) => self.finish(now, None),
                Ok(Some(body)) => {
                    let status = serde_json::from_slice(&body).ok();
                    self.finish(now, status.map(|status| (status, pending.local)));
                }
            },
            None if now >= self.next => match self.send(now) {
                Ok(pending) => self.pending = Some(pending),
                Err(_) => self.finish(now, None),
            },
            None => {}
        }
        self.issue
    }

    /// Abandon a check in flight, so the device's next request is ours alone.
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    #[cfg(test)]
    pub fn checking(&self) -> bool {
        self.pending.is_some()
    }

    fn send(&self, now: Instant) -> Result<Pending, RequestError> {
        let mut stream =
            TcpStream::connect_timeout(&self.device, CONNECT).map_err(RequestError::Connect)?;
        stream.set_write_timeout(Some(CONNECT))?;
        stream.write_all(self.request.as_bytes())?;
        stream.set_nonblocking(true)?;
        Ok(Pending {
            local: stream.local_addr()?.ip(),
            stream,
            reply: Vec::new(),
            deadline: now + REPLY,
        })
    }

    fn finish(&mut self, now: Instant, answer: Option<(Status, IpAddr)>) {
        self.next = now
            + if answer.is_some() {
                self.every
            } else {
                self.every * 5
            };
        self.issue = match answer {
            Some((status, local)) => self.judge(&status, local),
            None => Some(OutputIssue::Unreachable),
        };
    }

    fn judge(&mut self, status: &Status, local: IpAddr) -> Option<OutputIssue> {
        if !status.state.on {
            return Some(OutputIssue::Off);
        }
        if status.state.lor != 0 {
            return Some(OutputIssue::Overridden);
        }
        // `lm` is empty while live only through Prysm's JSON `live`: no DDP arrived.
        if !status.info.live || status.info.lm.as_deref() == Some("") {
            return Some(OutputIssue::NotLive);
        }
        let lip = status.info.lip.as_deref().and_then(|ip| ip.parse().ok())?;
        if lip == local || self.ours.is_none() {
            self.ours = Some(lip);
        }
        (self.ours != Some(lip)).then_some(OutputIssue::OtherSender)
    }
}

impl Pending {
    /// The body once the whole reply arrived; `None` while more is to come.
    fn read(&mut self) -> Result<Option<Vec<u8>>, RequestError> {
        let mut chunk = [0; 4096];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    return match http::parse(&self.reply, true)? {
                        Some(reply) => accept(reply).map(Some),
                        None => Err(RequestError::Malformed("connection closed mid-response")),
                    };
                }
                Ok(count) => {
                    self.reply.extend_from_slice(&chunk[..count]);
                    if self.reply.len() > MAX_REPLY {
                        return Err(RequestError::Malformed("state exceeds 64 KiB"));
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    return http::parse(&self.reply, false)?.map(accept).transpose();
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

fn accept((status, body): (u16, Vec<u8>)) -> Result<Vec<u8>, RequestError> {
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(RequestError::Status(status))
    }
}

/// The parts of WLED's `/json/si` that say whose colours it shows.
#[derive(Deserialize)]
struct Status {
    state: State,
    info: Info,
}

#[derive(Deserialize)]
struct State {
    on: bool,
    /// Live override: 0 off, 1 until live data ends, 2 until reboot.
    #[serde(default)]
    lor: u8,
}

#[derive(Deserialize)]
struct Info {
    /// Realtime data is shown (or was requested through the JSON API).
    live: bool,
    /// Realtime protocol, such as `"DDP"`; empty for the JSON API's `live`.
    #[serde(default)]
    lm: Option<String>,
    /// Address of the last realtime sender; empty when none.
    #[serde(default)]
    lip: Option<String>,
}
