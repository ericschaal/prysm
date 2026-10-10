use frames::{ViewFrame, Viewport};
use futures::{Stream, StreamExt};
use nodes::{BandDetector, EdgeSampler, TemporalSmoothing};
use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Config, EdgeColors};
use std::time::Instant;

mod frames;
mod nodes;

/// Stateful frame processor.
///
/// Frames stay in their raw capture format end-to-end; each node decodes
/// only the pixels it actually reads.
#[derive(Debug)]
pub struct PrysmProcessor {
    config: Config,
    // Pipeline nodes (in order)
    band_detector: Option<BandDetector>,
    sampler: EdgeSampler,
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
            config: config.clone(),
            band_detector: config.remove_black_bars.then(BandDetector::new),
            sampler: EdgeSampler::new(
                config.sample_density,
                f32::from(config.edge_depth_percent.clamp(1, 50)) / 100.0,
            ),
            temporal_smoothing: (config.smoothing_seconds.is_finite()
                && config.smoothing_seconds > 0.0)
                .then(|| TemporalSmoothing::new(config.smoothing_seconds)),
            last_viewport: None,
            frame_layout: None,
        }
    }

    /// Process a single frame using the current monotonic time for smoothing.
    pub fn process_frame(&mut self, frame: Frame) -> EdgeColors {
        self.process_frame_at(frame, Instant::now())
    }

    /// Process a frame at an explicit monotonic time, for deterministic replay.
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
            view = detector.process(view);
        }

        if self.last_viewport != Some(view.viewport)
            && let Some(smoother) = &mut self.temporal_smoothing
        {
            // Old samples describe a different region of the image.
            *smoother = TemporalSmoothing::new(self.config.smoothing_seconds);
        }

        self.last_viewport = Some(view.viewport);
        let mut edge_colors = self.sampler.process(&view);

        if let Some(ref mut smoother) = self.temporal_smoothing {
            edge_colors = smoother.process(edge_colors, now);
        }

        edge_colors * (f32::from(self.config.brightness_percent.min(100)) / 100.0)
    }

    /// Convert into a stream processor
    ///
    /// Consumes the processor and transforms a frame stream into an edge color stream.
    ///
    /// # Arguments
    /// * `input` - Input frame stream
    ///
    /// # Returns
    /// Stream of `EdgeColors`
    pub fn into_stream(
        mut self,
        input: impl Stream<Item = Frame> + Send + 'static,
    ) -> impl Stream<Item = EdgeColors> + Send + 'static {
        input.map(move |frame| self.process_frame(frame))
    }
}

impl Default for PrysmProcessor {
    fn default() -> Self {
        Self::new(&Config::default())
    }
}
