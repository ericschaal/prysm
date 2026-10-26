//! Desktop renderer configuration and construction.
use super::{DesktopRenderer, DisplayFrame, LayoutConfig};
use prysm_core::EdgeColors;
use std::{num::NonZeroU32, time::Duration};
use tokio_util::sync::CancellationToken;

pub struct DesktopRendererBuilder {
    layout_config: LayoutConfig,
    refresh_rate_hz: NonZeroU32,
    shutdown_token: Option<CancellationToken>,
    edge_colors_rx: tokio::sync::watch::Receiver<EdgeColors>,
    frame_rx: Option<tokio::sync::watch::Receiver<DisplayFrame>>,
}

impl std::fmt::Debug for DesktopRendererBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopRenderer")
            .field("shutdown_token", &"<CancellationToken>")
            .field("edge_colors_rx", &self.edge_colors_rx)
            .field("frame_rx", &self.frame_rx)
            .field("layout_config", &self.layout_config)
            .field("refresh_rate_hz", &self.refresh_rate_hz)
            .finish()
    }
}

impl DesktopRendererBuilder {
    #[must_use]
    pub fn new(edge_colors_rx: tokio::sync::watch::Receiver<EdgeColors>) -> Self {
        Self {
            frame_rx: None,
            shutdown_token: None,
            edge_colors_rx,
            layout_config: LayoutConfig::default(),
            refresh_rate_hz: NonZeroU32::new(30).unwrap(),
        }
    }

    #[must_use]
    pub fn with_frame_rx(mut self, frame_rx: tokio::sync::watch::Receiver<DisplayFrame>) -> Self {
        self.frame_rx = Some(frame_rx);
        self
    }

    /// Set the egui refresh rate (default: 30 Hz).
    #[must_use]
    pub fn with_refresh_rate_hz(mut self, refresh_rate_hz: NonZeroU32) -> Self {
        self.refresh_rate_hz = refresh_rate_hz;
        self
    }

    #[must_use]
    pub fn with_shutdown_token(mut self, token: &CancellationToken) -> Self {
        self.shutdown_token = Some(token.clone());
        self
    }

    /// Configure layout dimensions
    #[must_use]
    pub fn with_layout(mut self, config: LayoutConfig) -> Self {
        self.layout_config = config;
        self
    }

    /// Set LED strip width
    #[must_use]
    pub fn with_led_strip_width(mut self, width_px: f32) -> Self {
        self.layout_config.led_strip_width_px = width_px;
        self
    }

    /// Configure border
    #[must_use]
    pub fn with_border(mut self, width_px: f32, enabled: bool) -> Self {
        self.layout_config.border_width_px = width_px;
        self.layout_config.enable_border = enabled;
        self
    }

    /// Configure LED appearance
    #[must_use]
    pub fn with_led_size(mut self, size_px: f32) -> Self {
        self.layout_config.led_size_px = size_px;
        self
    }

    #[must_use]
    pub fn build(self) -> DesktopRenderer {
        DesktopRenderer {
            edge_colors_rx: self.edge_colors_rx,
            shutdown_token: self.shutdown_token,
            frame_rx: self.frame_rx,
            texture_handle: None,
            crop_rect: None,
            layout_config: self.layout_config,
            rgb_scratch: Vec::new(),
            refresh_interval: Duration::from_nanos(
                1_000_000_000u64.div_ceil(u64::from(self.refresh_rate_hz.get())),
            ),
        }
    }
}
