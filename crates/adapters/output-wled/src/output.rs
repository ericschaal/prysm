use crate::address::{Host, Names};
use crate::ddp::WledRenderer;
use crate::dither::Encoding;
use crate::http::{self, Deadline};
use crate::observe::{self, Observer};
use crate::{LedCounts, WledSettings};
use prysm_core::EdgeColors;
use prysm_ports::{
    Candidate, DeviceReport, Error, Hangup, IdleMode, LedOutput, OpenFailure, OutputFactory,
    Result, ValidationError, chain, system_lookup,
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

/// The most a session's stop waits for WLED. An open or an idle in flight
/// when the stop comes ends at once: the session cuts the `Hangup` it gave
/// the call (`prysm_ports::Hangup`), which ends its name resolution,
/// connect, request and reply. So a stop waits for its source's pull in
/// flight and close (`prysm_ports::SourceStop`), then for one idle, its own
/// (up to [`IDLE_LIMIT`]); that is the longest unless the source closes in
/// less than one idle. Two idles, without a pull or a close, are this
/// limit: a session that ended on its own and was idling `Off` (its own
/// idle, which nothing cuts) when the stop asked for its effects, then the
/// stop's own idle; the thread reads the stop's mode after closing the
/// source. `prysm_host::stops_in_time` says whether a host's stop deadline
/// covers every case; each host asserts it.
pub const STOP_WAIT: Duration = IDLE_LIMIT.saturating_add(IDLE_LIMIT);

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

    /// The device `settings` name, its host resolved within `wait` or
    /// until `stop` is cut.
    fn device(&self, settings: &WledSettings, wait: Duration, stop: &Hangup) -> Result<Device> {
        let address = settings.device()?;
        let ip = self.names.resolve(&address, wait, stop)?;
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
    /// at all, or `stop` was cut. All within [`OPEN_LIMIT`]: taking over by
    /// its first [`http::TIMEOUT`], turning off again by its end; a cut of
    /// `stop` ends it at once (a [`Stopping`](prysm_ports::Stopping) cause).
    /// A failure is [`OpenFailure::Untouched`] when nothing reached WLED:
    /// invalid settings, a name that does not resolve, the DDP socket, or a
    /// live request that could not connect.
    fn open(
        &self,
        settings: &WledSettings,
        stop: &Hangup,
    ) -> Result<Box<dyn LedOutput>, OpenFailure> {
        let deadline = Deadline::after(OPEN_LIMIT);
        let take_over = deadline.earlier(IDLE_LIMIT);
        let device = self
            .device(settings, take_over.remaining(), stop)
            .map_err(OpenFailure::Untouched)?;
        let ddp = SocketAddr::new(device.http.ip(), self.ddp_port);
        let renderer = WledRenderer::new(ddp).map_err(|source| {
            OpenFailure::Untouched(Error::adapter(
                format!("cannot open DDP socket to {ddp}"),
                source,
            ))
        })?;
        if let Err(live) = device.enter(Mode::Live, take_over, stop) {
            let context = device.context(Mode::Live);
            if !live.reached {
                return Err(OpenFailure::Untouched(Error::adapter(context, live.error)));
            }
            let live = live.error;
            // Whoever cut it puts the device in a mode next.
            if matches!(live, http::RequestError::Stopped(_)) {
                return Err(OpenFailure::Touched(Error::adapter(context, live)));
            }
            return Err(OpenFailure::Touched(
                match device.enter(Mode::Off, deadline, stop) {
                    Ok(()) => Error::adapter(context, live),
                    Err(off) => Error::adapter(
                        context,
                        NotTurnedOff {
                            live,
                            off: off.error,
                        },
                    ),
                },
            ));
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
        let never = Hangup::default();
        let device = self.device(settings, deadline.remaining(), &never)?;
        let mode = Mode::from(mode);
        device
            .enter(mode, deadline, &never)
            .map_err(|failed| Error::adapter(device.context(mode), failed.error))
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
    /// The one request that enters this mode. WLED stays in the realtime
    /// mode `live: true` put it in until it is told `live: false`, also once
    /// the colours stop, so leaving it is part of every other mode.
    fn state(self) -> Value {
        match self {
            Self::Live => json!({"on": true, "lor": 0, "live": true, "tt": 0}),
            Self::Off => json!({"live": false, "on": false, "lor": 2, "tt": 0}),
            Self::Native => json!({"live": false, "on": true, "lor": 2, "tt": 0}),
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

/// A mode change that failed: why, and whether its request may have reached
/// the device (anything but a request that could not connect).
struct NotEntered {
    error: http::RequestError,
    reached: bool,
}

impl NotEntered {
    /// `error`, which `reached` the device or not, or
    /// [`RequestError::Stopped`](http::RequestError::Stopped) once `stop`
    /// is cut, whatever the socket said.
    fn new(error: http::RequestError, reached: bool, stop: &Hangup) -> Self {
        let error = if stop.is_cut() {
            prysm_ports::Stopping.into()
        } else {
            error
        };
        Self { error, reached }
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
    /// Switch to `mode` with one request by `deadline`, or until `stop` is
    /// cut.
    fn enter(&self, mode: Mode, deadline: Deadline, stop: &Hangup) -> Result<(), NotEntered> {
        http::post_state(self.http, &self.host, &mode.state(), deadline, stop).map_err(|error| {
            let reached = !matches!(error, http::RequestError::Connect(_));
            NotEntered::new(error, reached, stop)
        })
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

    /// Within [`IDLE_LIMIT`], or until `stop` is cut.
    fn idle(mut self: Box<Self>, mode: IdleMode, stop: &Hangup) -> Result<()> {
        self.observer.cancel();
        let mode = Mode::from(mode);
        self.device
            .enter(mode, Deadline::after(IDLE_LIMIT), stop)
            .map_err(|failed| Error::adapter(self.device.context(mode), failed.error))
    }

    fn report(&mut self) -> DeviceReport {
        self.observer.report(Instant::now())
    }
}
