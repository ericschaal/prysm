//! Whether WLED is showing Prysm's colours, from its own state, how much its
//! own brightness dims them, how many LEDs it drives, and the gamma it
//! applies to them.
//!
//! One request at most every [`EVERY`] (5 × that after a check that got no
//! usable answer): `GET /json/si`, except that the second check and every
//! [`CONFIG_EVERY`]th after it read `GET /json/cfg` instead. The caller's
//! thread only connects (bounded by [`CONNECT`]) and sends the request; the
//! reply is read without blocking on later polls and abandoned after
//! [`REPLY`]. Nothing is sent to change WLED's state: Prysm reports, keeps
//! sending, and the LEDs follow again once WLED takes the colours.
//!
//! WLED shows realtime (DDP) colours at its master brightness `bri` scaled by
//! the LED preferences' brightness factor (`light.scale-bri`, percent,
//! capped at full), unless the realtime setting "Force max brightness"
//! (`if.live.maxbri`) is on: then at full brightness times that factor.
//! With "Use Gamma correction for brightness" on (`light.gc.bri`, reported
//! as 1 when off) it first passes the brightness through its gamma table
//! (`setBrightness`: `gamma8`, then `scaledBri`).
//!
//! It raises realtime colours to its colour gamma (`light.gc.col`, reported
//! as 1 when off) unless "Disable realtime gamma correction"
//! (`if.live.no-gc`) is on: then they are linear.
//!
//! Its LED count is the state check's `info.leds.count`: WLED's total
//! length, at no extra request.
//!
//! Without a configuration read (none yet, or WLED refuses it behind its
//! settings PIN) no brightness is reported and the gamma is unknown.
use crate::ddp::{Gamma, Transfer};
use crate::http::{self, RequestError};
use prysm_ports::{DeviceBrightness, OutputIssue, chain};
use serde::Deserialize;
use std::io::{self, ErrorKind, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

/// How often WLED's state is checked while it answers.
pub const EVERY: Duration = Duration::from_secs(1);
/// After a check without a usable answer the next waits this many times
/// [`EVERY`].
const BACKOFF: u32 = 5;
/// Longest the caller's thread waits, for the TCP connect (and the request
/// write, which fits an empty send buffer).
const CONNECT: Duration = Duration::from_millis(200);
/// How long a reply may take; it is read without waiting meanwhile.
const REPLY: Duration = Duration::from_secs(1);
/// `/json/si` grows with segments and usermods; WLED's own WebSocket limit.
const MAX_REPLY: usize = 64 * 1024;
/// Every this many checks one reads the configuration instead of the state
/// (the second check, then about every 30 s): what scales realtime colours
/// changes only when someone saves WLED's settings, so a change is reported
/// within half a minute at no extra request.
const CONFIG_EVERY: u32 = 30;
/// WLED's brightness scale: `bri` is 0–255.
const FULL: NonZeroU32 = NonZeroU32::new(255).unwrap();

/// What a check asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    State,
    Config,
}

pub struct Observer {
    device: SocketAddr,
    host: String,
    every: Duration,
    next: Instant,
    /// Checks started; picks which ask the next one is.
    checks: u32,
    /// `None`: only state checks.
    config_every: Option<u32>,
    pending: Option<Pending>,
    /// The realtime source address WLED names for Prysm's colours: our own
    /// address, or behind NAT (Docker's bridge network) the first one WLED
    /// named while it showed DDP.
    ours: Option<IpAddr>,
    issue: Option<OutputIssue>,
    /// `bri` from the latest state that answered; `None` after one that did
    /// not.
    bri: Option<u8>,
    /// `info.leds.count` from the latest state that answered; `None` after
    /// one that did not.
    leds: Option<u32>,
    /// What scales realtime colours, from the latest configuration read.
    config: Option<Realtime>,
}

struct Pending {
    ask: Ask,
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
            host: host.to_owned(),
            every,
            next: Instant::now() + every,
            checks: 0,
            config_every: Some(CONFIG_EVERY),
            pending: None,
            ours: None,
            issue: None,
            bri: None,
            leds: None,
            config: None,
        }
    }

    /// Only state checks, for tests that script every reply.
    #[cfg(test)]
    pub fn state_only(mut self) -> Self {
        self.config_every = None;
        self
    }

    /// The device's own brightness while it scales Prysm's colours, from
    /// the latest state and configuration; `None` until both answered.
    pub fn brightness(&self) -> Option<DeviceBrightness> {
        self.level()
            .map(|level| DeviceBrightness::of(level.into(), FULL))
    }

    /// The brightness WLED scales Prysm's colours by, 0–255 (its
    /// `scaledBri`), as [`Observer::brightness`] knows it.
    pub fn level(&self) -> Option<u8> {
        let config = self.config?;
        let bri = if config.force_max { u8::MAX } else { self.bri? };
        let bri = config.brightness_gamma.correct(bri);
        // The factor scales it, capped at full.
        let level = u32::from(bri) * u32::from(config.scale) / 100;
        Some(u8::try_from(level).unwrap_or(u8::MAX))
    }

    /// How many LEDs WLED drives, from the latest state; `None` until one
    /// answered and after one that did not.
    pub fn led_count(&self) -> Option<u32> {
        self.leds
    }

    /// How to encode colours for the gamma WLED applies, from the latest
    /// configuration read; sRGB until one gave the gamma.
    pub fn transfer(&self) -> Transfer {
        self.config
            .and_then(|config| config.gamma)
            .map_or(Transfer::Srgb, Transfer::Gamma)
    }

    /// Advance the check without waiting for WLED; the latest finding.
    pub fn poll(&mut self) -> Option<OutputIssue> {
        let now = Instant::now();
        if let Some(pending) = &mut self.pending {
            let body = match pending.read() {
                Ok(None) if now < pending.deadline => return self.issue,
                Ok(None) => Err(io::Error::from(ErrorKind::TimedOut).into()),
                Ok(Some(body)) => Ok(body),
                Err(error) => Err(error),
            };
            let (ask, local) = (pending.ask, pending.local);
            self.pending = None;
            self.answered(now, ask, body.map(|body| (body, local)));
        } else if now >= self.next {
            let ask = self.ask();
            self.checks = self.checks.wrapping_add(1);
            match self.send(now, ask) {
                Ok(pending) => self.pending = Some(pending),
                Err(error) => self.answered(now, ask, Err(error)),
            }
        }
        self.issue
    }

    /// The second check, then every [`CONFIG_EVERY`]th, reads the
    /// configuration; the rest read the state.
    fn ask(&self) -> Ask {
        match self.config_every {
            Some(every) if self.checks % every == 1 % every => Ask::Config,
            _ => Ask::State,
        }
    }

    fn answered(&mut self, now: Instant, ask: Ask, body: Result<(Vec<u8>, IpAddr), RequestError>) {
        match ask {
            Ask::State => self.finish(
                now,
                body.and_then(|(body, local)| Ok((serde_json::from_slice(&body)?, local))),
            ),
            Ask::Config => self.configured(
                now,
                body.and_then(|(body, _)| Ok(serde_json::from_slice::<Config>(&body)?)),
            ),
        }
    }

    /// Records a configuration read. A failure (WLED's settings PIN refuses
    /// it, say) keeps what an earlier read found and does not touch the
    /// state check's finding; its cause is logged at debug level.
    fn configured(&mut self, now: Instant, answer: Result<Config, RequestError>) {
        self.next = now
            + if answer.is_ok() {
                self.every
            } else {
                self.every * BACKOFF
            };
        match answer {
            Ok(config) => self.config = Some(config.into()),
            Err(error) => tracing::debug!(
                device = %self.device,
                error = %chain(&error),
                "WLED configuration read got no usable answer"
            ),
        }
    }

    /// Abandon a check in flight, so the device's next request is ours alone.
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    #[cfg(test)]
    pub fn checking(&self) -> bool {
        self.pending.is_some()
    }

    fn send(&self, now: Instant, ask: Ask) -> Result<Pending, RequestError> {
        let path = match ask {
            Ask::State => "/json/si",
            Ask::Config => "/json/cfg",
        };
        let request = http::get(path, &self.host);
        let mut stream =
            TcpStream::connect_timeout(&self.device, CONNECT).map_err(RequestError::Connect)?;
        stream.set_write_timeout(Some(CONNECT))?;
        stream.write_all(request.as_bytes())?;
        stream.set_nonblocking(true)?;
        Ok(Pending {
            ask,
            local: stream.local_addr()?.ip(),
            stream,
            reply: Vec::new(),
            deadline: now + REPLY,
        })
    }

    /// Records a check's outcome. Any failure, a reply that is not WLED's
    /// state included, reports [`OutputIssue::Unreachable`]; its cause is
    /// logged at debug level.
    fn finish(&mut self, now: Instant, answer: Result<(Status, IpAddr), RequestError>) {
        self.next = now
            + if answer.is_ok() {
                self.every
            } else {
                self.every * BACKOFF
            };
        let status = answer.as_ref().ok().map(|(status, _)| status);
        self.bri = status.and_then(|status| status.state.bri);
        self.leds = status
            .and_then(|status| status.info.leds)
            .map(|leds| leds.count);
        self.issue = match answer {
            Ok((status, local)) => self.judge(&status, local),
            Err(error) => {
                tracing::debug!(
                    device = %self.device,
                    error = %chain(&error),
                    "WLED state check got no usable answer"
                );
                Some(OutputIssue::Unreachable)
            }
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
        let lip = status.info.lip?;
        if lip == local || self.ours.is_none() {
            self.ours = Some(lip);
        }
        (self.ours != Some(lip)).then_some(OutputIssue::OtherSender)
    }
}

impl Pending {
    /// The body once the whole reply arrived; `None` while more is to come.
    fn read(&mut self) -> Result<Option<Vec<u8>>, RequestError> {
        http::read_response(&mut self.stream, &mut self.reply, MAX_REPLY)?
            .map(http::Response::success)
            .transpose()
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
    /// Master brightness, 0–255 (the last one set while off).
    #[serde(default)]
    bri: Option<u8>,
}

#[derive(Deserialize)]
struct Info {
    /// Realtime data is shown (or was requested through the JSON API).
    live: bool,
    /// Realtime protocol, such as `"DDP"`; empty for the JSON API's `live`.
    #[serde(default)]
    lm: Option<String>,
    /// Address of the last realtime sender; WLED sends it empty when none.
    #[serde(default, deserialize_with = "sender")]
    lip: Option<IpAddr>,
    #[serde(default)]
    leds: Option<Leds>,
}

/// An empty `lip` is no sender. Neither is one that is not an address (some
/// firmware may print a form `IpAddr` does not read): the rest of the reply
/// still counts.
fn sender<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<IpAddr>, D::Error> {
    let lip = Option::<String>::deserialize(deserializer)?;
    Ok(lip.and_then(|address| address.parse().ok()))
}

#[derive(Deserialize, Clone, Copy)]
struct Leds {
    /// WLED's total length: every LED on every output it drives.
    count: u32,
}

/// What scales realtime colours, and the gamma it raises them to.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Realtime {
    /// "Force max brightness": realtime colours ignore `bri`.
    force_max: bool,
    /// The brightness factor, percent; WLED's default is 100.
    scale: u8,
    /// The gamma WLED passes its brightness through; linear when off.
    brightness_gamma: Gamma,
    /// The colour gamma; `None` when the configuration does not give it.
    gamma: Option<Gamma>,
}

/// The parts of WLED's `/json/cfg` that scale realtime colours. A missing
/// field takes WLED's default, except the gamma, which is then unknown.
#[derive(Deserialize)]
struct Config {
    #[serde(rename = "if")]
    interfaces: Interfaces,
    #[serde(default)]
    light: Light,
}

#[derive(Deserialize)]
struct Interfaces {
    live: Live,
}

#[derive(Deserialize)]
struct Live {
    #[serde(default)]
    maxbri: bool,
    /// "Disable realtime gamma correction".
    #[serde(default, rename = "no-gc")]
    no_gamma: bool,
}

#[derive(Deserialize)]
#[serde(default)]
struct Light {
    #[serde(rename = "scale-bri")]
    scale: u8,
    #[serde(rename = "gc")]
    gamma: GammaCorrection,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            scale: 100,
            gamma: GammaCorrection::default(),
        }
    }
}

/// `light.gc`: the gamma for brightness and for colour, each reported as 1
/// when off. Brightness gamma is off by default.
#[derive(Deserialize, Default)]
#[serde(default)]
struct GammaCorrection {
    bri: Option<f32>,
    col: Option<f32>,
}

impl From<Config> for Realtime {
    fn from(config: Config) -> Self {
        let gamma = if config.interfaces.live.no_gamma {
            Some(Gamma::LINEAR)
        } else {
            config.light.gamma.col.and_then(Gamma::new)
        };
        Self {
            force_max: config.interfaces.live.maxbri,
            scale: config.light.scale,
            brightness_gamma: config
                .light
                .gamma
                .bri
                .and_then(Gamma::new)
                .unwrap_or(Gamma::LINEAR),
            gamma,
        }
    }
}
