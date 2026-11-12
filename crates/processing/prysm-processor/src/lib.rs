use frames::{FrameView, Viewport};
use futures::{Stream, StreamExt};
use nodes::{BlackBarDetector, SpatialSampler, TemporalSmoother};
use prysm_core::{Config, EdgeColors, Frame, PixelFormat};
use std::time::Instant;

mod frames;
mod nodes;

/// Stateful frame processor.
///
/// Detection keeps the raw capture format; sampling reads RGB and NV12 in place
/// (NV12 per row, without a full-frame conversion) and converts YUYV into reusable
/// RGB storage, before reducing the picture to a grid and projecting onto the perimeter.
/// Pictures above 640x360 are averaged on a lattice of every nth pixel and row
/// and get the grid of 640x360, so sampling costs about the same at any capture size.
#[derive(Debug)]
pub struct PrysmProcessor {
    config: Config,
    // Pipeline nodes (in order)
    black_bar_detector: Option<BlackBarDetector>,
    sampler: SpatialSampler,
    temporal_smoother: Option<TemporalSmoother>,
    last_viewport: Option<Viewport>,
    frame_layout: Option<FrameLayout>,
}

/// A layout change invalidates geometry and temporal history.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FrameLayout {
    width: u32,
    height: u32,
    format: PixelFormat,
}

impl PrysmProcessor {
    pub fn new(config: &Config) -> Self {
        Self {
            config: *config,
            black_bar_detector: config.remove_black_bars.then(BlackBarDetector::new),
            sampler: SpatialSampler::new(
                config.sample_density,
                config.spatial_reach_percent.as_ratio(),
            ),
            temporal_smoother: config
                .smoothing_seconds
                .is_enabled()
                .then(|| TemporalSmoother::new(config.smoothing_seconds)),
            last_viewport: None,
            frame_layout: None,
        }
    }

    /// Confirmed sampling area of the last supported frame, in source pixels.
    #[must_use]
    pub fn viewport(&self) -> Option<Viewport> {
        self.last_viewport
    }

    /// Apply processing settings without discarding unrelated temporal history.
    /// Call this when configuration changes, before processing the next frame.
    pub fn configure(&mut self, config: &Config) {
        self.sampler.configure(
            config.sample_density,
            config.spatial_reach_percent.as_ratio(),
        );
        if config.remove_black_bars != self.config.remove_black_bars {
            self.black_bar_detector = config.remove_black_bars.then(BlackBarDetector::new);
        }
        if config.smoothing_seconds.is_enabled() {
            if let Some(smoother) = &mut self.temporal_smoother {
                smoother.configure(config.smoothing_seconds);
            } else {
                self.temporal_smoother = Some(TemporalSmoother::new(config.smoothing_seconds));
            }
        } else {
            self.temporal_smoother = None;
        }
        self.config = *config;
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
        let layout = FrameLayout {
            width: frame.width(),
            height: frame.height(),
            format: frame.format(),
        };
        if self.frame_layout != Some(layout) {
            *self = Self::new(&self.config);
            self.frame_layout = Some(layout);
        }

        if frame.width() == 0 || frame.height() == 0 || frame.format() == PixelFormat::MJPEG {
            tracing::error!(
                "Unsupported frame: {} {}x{}",
                frame.format(),
                frame.width(),
                frame.height()
            );
            return EdgeColors::black(
                frame.width() as usize,
                frame.height() as usize,
                self.config.sample_density,
            );
        }

        let mut view = FrameView::new(frame);

        if let Some(ref mut detector) = self.black_bar_detector {
            view = detector.process(view, now);
        }

        self.last_viewport = Some(view.viewport);
        let mut edge_colors = self.sampler.process(&view);

        if let Some(ref mut smoother) = self.temporal_smoother {
            edge_colors = smoother.process(edge_colors, now);
        }

        if self.config.correction != prysm_core::ColorCorrection::default()
            || self.config.shadow_lift_percent.0 != 0
            || self.config.brightness_percent.0 < 100
        {
            for strip in [
                &mut edge_colors.top,
                &mut edge_colors.right,
                &mut edge_colors.bottom,
                &mut edge_colors.left,
            ] {
                strip.map_colors(|color| {
                    self.config
                        .correction
                        .apply(color.lift_shadows(self.config.shadow_lift_percent))
                        * self.config.brightness_percent.as_ratio()
                });
            }
        }
        edge_colors
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

#[cfg(test)]
mod correction_tests {
    use super::*;
    #[test]
    fn live_profile_changes_precede_brightness_and_do_not_retain_previous_correction() {
        let mut config = Config {
            remove_black_bars: false,
            smoothing_seconds: prysm_core::SmoothingSeconds(0.0),
            brightness_percent: prysm_core::BrightnessPercent(70),
            ..Config::default()
        };
        let frame = Frame::fill(128, 4, 2, PixelFormat::RGB24).unwrap();
        let mut processor = PrysmProcessor::new(&config);
        let original = processor.process_frame(frame.clone()).top.sample_at(0.5);
        config.correction = prysm_core::ColorCorrection::new(60, -30, 40).unwrap();
        processor.configure(&config);
        let corrected = processor.process_frame(frame.clone()).top.sample_at(0.5);
        let expected = config.correction.apply(original / 0.7) * 0.7;
        assert!((corrected.r - expected.r).abs() < 1e-6);
        assert!((corrected.g - expected.g).abs() < 1e-6);
        assert!((corrected.b - expected.b).abs() < 1e-6);
        config.correction = prysm_core::ColorCorrection::default();
        processor.configure(&config);
        assert_eq!(processor.process_frame(frame).top.sample_at(0.5), original);
    }
}
