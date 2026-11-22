use crate::address::{Host, Names};
use crate::ddp::WledRenderer;
use crate::dither::Encoding;
use crate::http::{self, Deadline};
use crate::observe::{self, Observer};
use crate::{LedCounts, WledSettings};
use prysm_core::EdgeColors;
use prysm_ports::{
    Candidate, DeviceReport, Error, IdleMode, LedOutput, OutputFactory, Result, ValidationError,
    chain, system_lookup,
};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// WLED's DDP port (UDP).
const DDP_PORT: u16 = 4048;

/// The longest any idle takes, one deadline for all of it: a held output's
/// mode change, or for a device nobody holds ([`Wled::idle`]) its name
/// resolution and mode change.
pub const IDLE_LIMIT: Duration = http::TIMEOUT;

/// The longest an open takes, one deadline for all of it: name resolution
/// and live mode by [`http::TIMEOUT`] after it starts, then, if live mode
/// failed, switching the device off again within [`IDLE_LIMIT`].
pub const OPEN_LIMIT: Duration = http::TIMEOUT.saturating_add(IDLE_LIMIT);

// What a session's stop waits for, by these limits: the source's close,
// then the output's idle (`IDLE_LIMIT`); and when the stop comes while the
// session opens WLED at a frame, that open first (`OPEN_LIMIT`). With each
// host's stop deadline and its source's release limit, which live in the
// hosts and the capture adapters (none of these sums is asserted):
//
// | Host and source | Stop deadline | Release | Close and idle | With an open in flight |
// | --- | --- | --- | --- | --- |
// | TV, `webos` | 6 s | 4 s | 5.5 s | 8.5 s: exceeds the stop deadline; the engine then faults |
// | Desktop, `device` | 10 s | 3 s | 4.5 s | 7.5 s |
// | Desktop, `file` | 10 s | 5 s (exit 2 s, reap 3 s) | 6.5 s | 9.5 s |
// | Desktop, `remote` | 10 s | 5.05 s | 6.55 s | 9.55 s |
//
// Every other row fits its stop deadline. The TV's open-in-flight case
// fits only with a stop deadline above 8.5 s, or with an open that ends
// when the stop comes (then 5.5 s).

/// Opens WLED outputs. Enable DDP reception in WLED's Sync Interfaces settings.
#[derive(Debug)]
pub struct Wled {
    ddp_port: u16,
    /// How often an open output checks WLED's state.
    check_every: Duration,
    names: Names,
}

impl Default for Wled {
    fn default() -> Self {
        Self {
            ddp_port: DDP_PORT,
            check_every: observe::EVERY,
            names: Names::new(system_lookup),
        }
    }
}

impl Wled {
    /// Checks the state at every `observe`.
    #[cfg(test)]
    pub(crate) fn local(ddp_port: u16) -> Self {
        Self {
            ddp_port,
            check_every: Duration::ZERO,
            names: Names::new(system_lookup),
        }
    }

    /// Resolves names with `lookup` instead of the system resolver.
    #[cfg(test)]
    pub(crate) fn with_lookup(mut self, lookup: prysm_ports::Lookup) -> Self {
        self.names = Names::new(lookup);
        self
    }

    /// The device `settings` name, its host resolved within `wait`.
    fn device(&self, settings: &WledSettings, wait: Duration) -> Result<Device> {
        settings.validate()?;
        let address = settings.device()?;
        let ip = self.names.resolve(&address, wait)?;
        let label = match address.host {
            Host::Ip(_) => address.to_string(),
            Host::Name(_) => format!("{address} ({ip})"),
        };
        Ok(Device {
            http: SocketAddr::new(ip, address.port),
            host: address.to_string(),
            label,
        })
    }
}

impl OutputFactory for Wled {
    const KIND: &'static str = "wled";
    type Settings = WledSettings;

    fn validate(&self, settings: &WledSettings) -> Result<(), ValidationError> {
        settings.validate()
    }

    /// WLED devices answering on the local network, each filling `address`;
    /// about 1.5 s at most. See `discover`.
    fn candidates(&self) -> Vec<Candidate> {
        crate::discover::Browse::default().candidates()
    }

    /// Resolves the host, then takes the device over for live colours. On
    /// failure the device is turned off again unless it could not be reached
    /// at all. All within [`OPEN_LIMIT`]: taking over by its first
    /// [`http::TIMEOUT`], turning off again by its end.
    fn open(&self, settings: &WledSettings) -> Result<Box<dyn LedOutput>> {
        let deadline = Deadline::after(OPEN_LIMIT);
        let take_over = deadline.earlier(IDLE_LIMIT);
        let device = self.device(settings, take_over.remaining())?;
        let ddp = SocketAddr::new(device.http.ip(), self.ddp_port);
        let renderer = WledRenderer::new(ddp)
            .map_err(|source| Error::adapter(format!("cannot open DDP socket to {ddp}"), source))?;
        if let Err(live) = device.enter(Mode::Live, take_over) {
            let context = device.context(Mode::Live);
            if matches!(live, http::RequestError::Connect(_)) {
                return Err(Error::adapter(context, live));
            }
            return Err(match device.enter(Mode::Off, deadline) {
                Ok(()) => Error::adapter(context, live),
                Err(off) => Error::adapter(context, NotTurnedOff { live, off }),
            });
        }
        Ok(Box::new(WledOutput {
            observer: Observer::new(device.http, &device.host, self.check_every, Instant::now()),
            device,
            renderer,
            leds: settings.leds,
            encoding: Encoding::new(settings.dither),
        }))
    }

    /// Resolves the host and sets the mode, without live mode first: the
    /// device never shows Prysm's colours meanwhile. Within [`IDLE_LIMIT`].
    fn idle(&self, settings: &WledSettings, mode: IdleMode) -> Result<()> {
        let deadline = Deadline::after(IDLE_LIMIT);
        let device = self.device(settings, deadline.remaining())?;
        let mode = Mode::from(mode);
        device
            .enter(mode, deadline)
            .map_err(|source| Error::adapter(device.context(mode), source))
    }
}

/// Live mode failed, and turning the device off again failed too. Both
/// causes are kept; neither is the other's source, so each is rendered in
/// its own clause.
#[derive(Debug, thiserror::Error)]
#[error("{}; turning it off also failed: {}", chain(.live), chain(.off))]
pub(crate) struct NotTurnedOff {
    pub(crate) live: http::RequestError,
    pub(crate) off: http::RequestError,
}

#[derive(Debug, Clone, Copy)]
enum Mode {
    Live,
    Off,
    Native,
}

impl Mode {
    fn state(self) -> Value {
        match self {
            Self::Live => json!({"on": true, "lor": 0, "live": true, "tt": 0}),
            Self::Off => json!({"on": false, "lor": 2, "tt": 0}),
            Self::Native => json!({"on": true, "lor": 2, "tt": 0}),
        }
    }
}

impl From<IdleMode> for Mode {
    fn from(mode: IdleMode) -> Self {
        match mode {
            IdleMode::Off => Self::Off,
            IdleMode::Native => Self::Native,
        }
    }
}

/// A WLED device's JSON API, resolved.
struct Device {
    /// The resolved IP; the HTTP port from the address.
    http: SocketAddr,
    /// `Host` header: the configured `host[:port]`.
    host: String,
    /// The device in messages: the configured address, plus the IP for a name.
    label: String,
}

impl Device {
    /// Switch to `mode` by `deadline`, both requests included. A previous
    /// realtime session can ignore `lor` until it is released, so live mode
    /// is left first.
    fn enter(&self, mode: Mode, deadline: Deadline) -> Result<(), http::RequestError> {
        http::post_state(self.http, &self.host, &json!({"live": false}), deadline)?;
        http::post_state(self.http, &self.host, &mode.state(), deadline)
    }

    fn context(&self, mode: Mode) -> String {
        let mode = match mode {
            Mode::Live => "live",
            Mode::Off => "off",
            Mode::Native => "native",
        };
        format!("WLED at {} did not switch to {mode} mode", self.label)
    }
}

struct WledOutput {
    device: Device,
    renderer: WledRenderer,
    /// Validated at open.
    leds: LedCounts,
    /// Follows WLED's gamma and brightness at every write.
    encoding: Encoding,
    observer: Observer,
}

impl LedOutput for WledOutput {
    fn write(&mut self, colors: &EdgeColors) -> Result<()> {
        self.encoding
            .follow(self.observer.transfer(), self.observer.level());
        self.renderer
            .render_edges(colors, self.leds, &self.encoding, Instant::now())
            .map_err(|source| {
                Error::adapter(
                    format!("cannot send colours to WLED at {}", self.device.label),
                    source,
                )
            })
    }

    /// Within [`IDLE_LIMIT`].
    fn idle(mut self: Box<Self>, mode: IdleMode) -> Result<()> {
        self.observer.cancel();
        let mode = Mode::from(mode);
        self.device
            .enter(mode, Deadline::after(IDLE_LIMIT))
            .map_err(|source| Error::adapter(self.device.context(mode), source))
    }

    fn report(&mut self) -> DeviceReport {
        self.observer.report(Instant::now())
    }
}
