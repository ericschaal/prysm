use crate::address::{Host, Names};
use crate::classify::{self, Cause};
use crate::ddp::{WledRenderer, route_address};
use crate::dither::Encoding;
use crate::http::{self, Deadline};
use crate::lifecycle::{self, WledObservation};
use crate::observe::{self, Observer};
use crate::{LedCounts, WledSettings};
use prysm_core::EdgeColors;
use prysm_ports::{
    Candidate, DeviceReport, Error, Hangup, IdleMode, LedOutput, OpenFailure, OpenIntent,
    OutputDisplay, OutputFactory, OutputObservationState, OutputOpen, OutputRelease, Power,
    Readiness, Result, ValidationError, system_lookup,
};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// WLED's DDP port (UDP).
const DDP_PORT: u16 = 4048;

/// The deadline for a device observation, explicit Off, or ordinary release.
pub const IDLE_LIMIT: Duration = http::TIMEOUT;

/// Resolution and observation, then a command within the remaining time.
pub const OPEN_LIMIT: Duration = http::TIMEOUT.saturating_add(IDLE_LIMIT);

/// The session can finish a release or an Off command within this bound.
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
            ddp: SocketAddr::new(ip, self.ddp_port),
            host: address.to_string(),
            label,
        })
    }
    fn renderer(device: &Device) -> Result<WledRenderer, OpenFailure> {
        let ddp = device.ddp;
        WledRenderer::new(ddp).map_err(|source| {
            OpenFailure::Untouched(Error::adapter_escalating(
                format!("cannot open DDP socket to {ddp}"),
                source,
                classify::escalation(Cause::Socket),
            ))
        })
    }

    fn streaming(
        &self,
        settings: &WledSettings,
        device: Device,
        renderer: WledRenderer,
    ) -> Box<dyn LedOutput> {
        Box::new(WledOutput {
            observer: Observer::new(
                device.http,
                device.ddp,
                &device.host,
                self.check_every,
                Instant::now(),
            ),
            device,
            renderer,
            leds: settings.leds,
            encoding: Encoding::new(settings.dither),
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

    fn supports_effects(&self) -> bool {
        true
    }

    fn observe(
        &self,
        settings: &WledSettings,
        stop: &Hangup,
    ) -> (Readiness<Error>, OutputObservationState) {
        let deadline = Deadline::after(IDLE_LIMIT);
        let observed = self
            .device(settings, deadline.remaining(), stop)
            .and_then(|device| device.observe(deadline, stop));
        match observed {
            Ok(state) => (
                Readiness::Ready,
                OutputObservationState::Observed(state.output()),
            ),
            Err(error) => (self.failure(error), OutputObservationState::Pending),
        }
    }

    fn failure(&self, error: Error) -> Readiness<Error> {
        lifecycle::readiness_failure(error)
    }

    fn watch(
        &self,
        settings: &WledSettings,
        stop: &Hangup,
    ) -> Option<(Readiness<Error>, OutputObservationState)> {
        while !stop.wait(observe::EVERY) {
            let (readiness, observation) = self.observe(settings, stop);
            if !matches!(readiness, Readiness::Waiting(_)) {
                return (!stop.is_cut()).then_some((readiness, observation));
            }
        }
        None
    }

    fn open_for_sync(
        &self,
        settings: &WledSettings,
        intent: OpenIntent,
        stop: &Hangup,
    ) -> Result<OutputOpen, OpenFailure> {
        let deadline = Deadline::after(OPEN_LIMIT);
        let readiness = deadline.earlier(IDLE_LIMIT);
        let device = self
            .device(settings, readiness.remaining(), stop)
            .map_err(OpenFailure::Untouched)?;
        let command = match intent {
            OpenIntent::Explicit => json!({"on": true, "lor": 0, "tt": 0}),
            OpenIntent::Resume => {
                let observed = device
                    .observe(readiness, stop)
                    .map_err(OpenFailure::Untouched)?
                    .output();
                if observed.display == OutputDisplay::Override {
                    return Ok(OutputOpen::Handoff(observed));
                }
                json!({"on": true, "tt": 0})
            }
        };
        let renderer = Self::renderer(&device)?;
        device.apply("live", &command, deadline, stop)?;
        Ok(OutputOpen::Streaming(
            self.streaming(settings, device, renderer),
        ))
    }

    fn set_power(&self, settings: &WledSettings, power: Power, stop: &Hangup) -> Result<()> {
        let timeout = match power {
            Power::Off => IDLE_LIMIT,
            Power::On => OPEN_LIMIT,
        };
        let deadline = Deadline::after(timeout);
        let device = self.device(settings, deadline.remaining(), stop)?;
        let (command, mode) = match power {
            Power::Off => (power_off_command(), "off"),
            Power::On => {
                let status = device.observe(deadline.earlier(IDLE_LIMIT), stop)?;
                let Some(command) = lifecycle::effects_power_command(status) else {
                    return Ok(());
                };
                (command, "native")
            }
        };
        device
            .apply(mode, &command, deadline, stop)
            .map_err(OpenFailure::into_error)
    }
}

fn power_off_command() -> Value {
    json!({"on": false, "live": false, "tt": 0})
}

/// `error` of a request under `context`, escalating by its row.
fn request_failed(context: String, error: http::RequestError) -> Error {
    let escalation = classify::escalation(Cause::Request(&error));
    Error::adapter_escalating(context, error, escalation)
}

/// A WLED device's JSON API, resolved.
struct Device {
    /// The resolved IP; the HTTP port from the address.
    http: SocketAddr,
    ddp: SocketAddr,
    /// `Host` header: the configured `host[:port]`.
    host: String,
    /// The device in messages: the configured address, plus the IP for a name.
    label: String,
}

impl Device {
    /// Classify whether the command could have reached WLED before a cut.
    fn apply(
        &self,
        mode: &str,
        state: &Value,
        deadline: Deadline,
        stop: &Hangup,
    ) -> Result<(), OpenFailure> {
        http::post_state(self.http, &self.host, state, deadline, stop).map_err(|error| {
            let untouched = matches!(error, http::RequestError::Connect(_));
            let cause = if stop.is_cut() {
                prysm_ports::Stopping.into()
            } else {
                error
            };
            let error = request_failed(
                format!("WLED at {} did not switch to {mode} mode", self.label),
                cause,
            );
            if untouched {
                OpenFailure::Untouched(error)
            } else {
                OpenFailure::Touched(error)
            }
        })
    }

    fn observe(&self, deadline: Deadline, stop: &Hangup) -> Result<WledObservation> {
        http::get_body(self.http, &self.host, "/json/si", deadline, stop)
            .and_then(|state| Ok(WledObservation::parse(&state)?))
            .map_err(|error| {
                request_failed(format!("cannot read WLED state at {}", self.label), error)
            })
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
            .render_edges(colors, self.leds, &self.encoding)
            .map_err(|source| {
                Error::adapter_escalating(
                    format!("cannot send colours to WLED at {}", self.device.label),
                    source,
                    classify::escalation(Cause::Send),
                )
            })
    }

    fn release(mut self: Box<Self>, release: OutputRelease, stop: &Hangup) -> Result<()> {
        self.observer.cancel();
        let own_sender = self.observer.known_sender();
        let WledOutput {
            device, renderer, ..
        } = *self;
        drop(renderer);
        if release == OutputRelease::Local {
            return Ok(());
        }
        let deadline = Deadline::after(IDLE_LIMIT);
        let status = device.observe(deadline.earlier(IDLE_LIMIT / 2), stop);
        let own_sender = own_sender.filter(|sender| route_address(device.ddp) == Some(*sender));
        if !status
            .is_ok_and(|state| state.during_stream(own_sender) == lifecycle::StreamDisplay::Ours)
        {
            return Ok(());
        }
        let (command, mode) = match release {
            OutputRelease::Ordinary(IdleMode::Off) => (power_off_command(), "off"),
            OutputRelease::Ordinary(IdleMode::Native) | OutputRelease::Handoff => {
                (json!({"live": false}), "native")
            }
            OutputRelease::Local => return Ok(()),
        };
        device
            .apply(mode, &command, deadline, stop)
            .map_err(OpenFailure::into_error)
    }

    fn stream_observation(&self) -> OutputObservationState {
        self.observer
            .observation()
            .map_or(OutputObservationState::Pending, |state| {
                OutputObservationState::Observed(state.output())
            })
    }

    fn report(&mut self) -> DeviceReport {
        self.observer.report(Instant::now())
    }
}
