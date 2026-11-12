use crate::address::Host;
use crate::{WledSettings, http};
use prysm_core::EdgeColors;
use prysm_ports::{Error, IdleMode, LedOutput, OutputFactory, Result, ValidationError};
use prysm_renderer_wled::WledRenderer;
use serde_json::{Value, json};
use std::net::SocketAddr;

/// Opens WLED outputs. Enable DDP reception in WLED's Sync Interfaces settings.
#[derive(Debug, Clone, Copy)]
pub struct Wled {
    ddp_port: u16,
}

impl Default for Wled {
    fn default() -> Self {
        Self { ddp_port: 4048 }
    }
}

impl Wled {
    #[cfg(test)]
    pub(crate) fn local(ddp_port: u16) -> Self {
        Self { ddp_port }
    }
}

impl OutputFactory for Wled {
    const KIND: &'static str = "wled";
    type Settings = WledSettings;

    fn validate(&self, settings: &WledSettings) -> Result<(), ValidationError> {
        settings.validate()
    }

    /// Resolves the host, then takes the device over for live colours. On
    /// failure the device is turned off again unless it could not be reached
    /// at all.
    fn open(&self, settings: &WledSettings) -> Result<Box<dyn LedOutput>> {
        settings.validate()?;
        let address = settings.device()?;
        let ip = address.resolve(http::TIMEOUT)?;
        let ddp = SocketAddr::new(ip, self.ddp_port);
        let renderer = WledRenderer::new(ddp)
            .map_err(|source| Error::adapter(format!("cannot open DDP socket to {ddp}"), source))?;
        let label = match address.host {
            Host::Ip(_) => address.to_string(),
            Host::Name(_) => format!("{address} ({ip})"),
        };
        let output = WledOutput {
            device: SocketAddr::new(ip, address.port),
            host: address.to_string(),
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

    fn idle(self: Box<Self>, mode: IdleMode) -> Result<()> {
        let mode = match mode {
            IdleMode::Off => Mode::Off,
            IdleMode::Native => Mode::Native,
        };
        self.enter(mode)
            .map_err(|source| Error::adapter(self.context(mode), source))
    }
}
