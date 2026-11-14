use crate::address::{Host, Names};
use crate::ddp::WledRenderer;
use crate::observe::{self, Observer};
use crate::{WledSettings, http};
use prysm_core::EdgeColors;
use prysm_ports::{
    Candidate, Error, IdleMode, LedOutput, OutputFactory, OutputIssue, Result, ValidationError,
    system_lookup,
};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;

/// WLED's DDP port (UDP).
const DDP_PORT: u16 = 4048;

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
    /// at all.
    fn open(&self, settings: &WledSettings) -> Result<Box<dyn LedOutput>> {
        settings.validate()?;
        let address = settings.device()?;
        let ip = self.names.resolve(&address, http::TIMEOUT)?;
        let ddp = SocketAddr::new(ip, self.ddp_port);
        let renderer = WledRenderer::new(ddp)
            .map_err(|source| Error::adapter(format!("cannot open DDP socket to {ddp}"), source))?;
        let label = match address.host {
            Host::Ip(_) => address.to_string(),
            Host::Name(_) => format!("{address} ({ip})"),
        };
        let device = SocketAddr::new(ip, address.port);
        let host = address.to_string();
        let output = WledOutput {
            observer: Observer::new(device, &host, self.check_every),
            device,
            host,
            label,
            renderer,
            leds: settings.leds.edges(),
        };
        match output.enter(Mode::Live) {
            Ok(()) => Ok(Box::new(output)),
            Err(source) => {
                let mut context = output.context(Mode::Live);
                if !matches!(source, http::RequestError::Connect(_))
                    && let Err(cleanup) = output.enter(Mode::Off)
                {
                    context = format!("{context}; turning it off also failed: {cleanup}");
                }
                Err(Error::adapter(context, source))
            }
        }
    }
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

struct WledOutput {
    /// Resolved at open; HTTP port from the address.
    device: SocketAddr,
    /// `Host` header: the configured `host[:port]`.
    host: String,
    /// The device in messages: the configured address, plus the IP for a name.
    label: String,
    renderer: WledRenderer,
    leds: [usize; 4],
    observer: Observer,
}

impl WledOutput {
    /// A previous realtime session can ignore `lor` until it is released.
    fn enter(&self, mode: Mode) -> Result<(), http::RequestError> {
        http::post_state(self.device, &self.host, &json!({"live": false}))?;
        http::post_state(self.device, &self.host, &mode.state())
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

impl LedOutput for WledOutput {
    fn write(&mut self, colors: &EdgeColors) -> Result<()> {
        self.renderer
            .render_edges(colors, self.leds)
            .map_err(|source| {
                Error::adapter(
                    format!("cannot send colours to WLED at {}", self.label),
                    source,
                )
            })
    }

    fn idle(mut self: Box<Self>, mode: IdleMode) -> Result<()> {
        self.observer.cancel();
        let mode = match mode {
            IdleMode::Off => Mode::Off,
            IdleMode::Native => Mode::Native,
        };
        self.enter(mode)
            .map_err(|source| Error::adapter(self.context(mode), source))
    }

    fn observe(&mut self) -> Option<OutputIssue> {
        self.observer.poll()
    }
}
