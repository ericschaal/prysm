use frames::{ViewFrame, Viewport};
use futures::{Stream, StreamExt};
use nodes::{BandDetector, SpatialSampler, TemporalSmoothing};
use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Config, EdgeColors};
use std::time::Instant;

mod frames;
mod nodes;

/// Stateful frame processor.
///
/// Frames stay in their raw capture format; the sampler reduces the active
/// picture directly to a small linear RGB grid before projecting onto the perimeter.
#[derive(Debug)]
pub struct PrysmProcessor {
    config: Config,
    // Pipeline nodes (in order)
    band_detector: Option<BandDetector>,
    sampler: SpatialSampler,
    temporal_smoothing: Option<TemporalSmoothing>,
    last_viewport: Option<Viewport>,
    frame_layout: Option<(
        u32,
        u32,
        PixelFormat,
        prysm_capture::YuvRange,
        prysm_capture::YuvStandardMatrix,
    )>,
}

impl PrysmProcessor {
    pub fn new(config: &Config) -> Self {
        Self {
            config: *config,
            band_detector: config.remove_black_bars.then(BandDetector::new),
            sampler: SpatialSampler::new(
                config.sample_density,
                config.spatial_reach_percent.as_ratio(),
            ),
            temporal_smoothing: config
                .smoothing_seconds
                .is_enabled()
                .then(|| TemporalSmoothing::new(config.smoothing_seconds)),
            last_viewport: None,
            frame_layout: None,
        }
    }

    /// Confirmed sampling area of the last supported frame, in source pixels.
    pub fn viewport(&self) -> Option<Viewport> {
        self.last_viewport
    }

    /// Process a single frame using the current monotonic time for smoothing.
    pub fn process_frame(&mut self, frame: Frame) -> EdgeColors {
        self.process_frame_at(frame, Instant::now())
    }

    /// Process a frame at an explicit monotonic time, for deterministic replay.
    /// Both black-bar confirmation and smoothing use this clock.
    /// Use a shared clock origin plus source timestamps when replaying video.
    /// Repeated or earlier times do not advance smoothing.
    pub fn process_frame_at(&mut self, frame: Frame, now: Instant) -> EdgeColors {
        let layout = (
            frame.width,
            frame.height,
            frame.format,
            frame.yuv_range,
            frame.yuv_matrix,
        );
        if self.frame_layout != Some(layout) {
            *self = Self::new(&self.config);
            self.frame_layout = Some(layout);
        }

        if frame.width == 0 || frame.height == 0 || frame.format == PixelFormat::MJPEG {
            tracing::error!(
                "Unsupported frame: {} {}x{}",
                frame.format,
                frame.width,
                frame.height
            );
            return EdgeColors::black(
                frame.width as usize,
                frame.height as usize,
                self.config.sample_density,
            );
        }

        let mut view = ViewFrame::new(frame);

        if let Some(ref mut detector) = self.band_detector {
            view = detector.process(view, now);
        }

        self.last_viewport = Some(view.viewport);
        let mut edge_colors = self.sampler.process(&view);

        if let Some(ref mut smoother) = self.temporal_smoothing {
            edge_colors = smoother.process(edge_colors, now);
        }

        edge_colors * self.config.brightness_percent.as_ratio()
    }

    /// Convert into a stream processor
    ///
    /// Consumes the processor and transforms a frame stream into an edge color stream.
    ///
    /// # Arguments
    /// * `input` - Input frame stream
    ///
    /// # Returns
    /// Stream of `EdgeColors`, preserving source errors
    pub fn into_stream<E: Send + 'static>(
        mut self,
        input: impl Stream<Item = Result<Frame, E>> + Send + 'static,
    ) -> impl Stream<Item = Result<EdgeColors, E>> + Send + 'static {
        input.map(move |frame| frame.map(|frame| self.process_frame(frame)))
    }
}

impl Default for PrysmProcessor {
    fn default() -> Self {
        Self::new(&Config::default())
    }
}
