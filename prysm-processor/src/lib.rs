use frames::{ViewFrame, Viewport};
use futures::{Stream, StreamExt};
use nodes::{BandDetector, EdgeSampler, TemporalSmoothing};
use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Config, EdgeSpectra, SampleDensity};

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
                SampleDensity::default(),
                f32::from(config.edge_depth_percent.clamp(1, 50)) / 100.0,
            ),
            temporal_smoothing: (config.smoothing_percent > 0)
                .then(|| TemporalSmoothing::new(f32::from(config.smoothing_percent) / 100.0)),
            last_viewport: None,
            frame_layout: None,
        }
    }

    /// Process a single frame through the pipeline
    pub fn process_frame(&mut self, frame: Frame) -> EdgeSpectra {
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
            return EdgeSpectra::black(
                frame.width as usize,
                frame.height as usize,
                SampleDensity::default(),
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
            *smoother = TemporalSmoothing::new(f32::from(self.config.smoothing_percent) / 100.0);
        }

        self.last_viewport = Some(view.viewport);
        let mut spectra = self.sampler.process(&view);

        if let Some(ref mut smoother) = self.temporal_smoothing {
            spectra = smoother.process(spectra);
        }

        spectra * (f32::from(self.config.brightness_percent.min(100)) / 100.0)
    }

    /// Convert into a stream processor
    ///
    /// Consumes the processor and transforms a frame stream into an edge spectrum stream.
    ///
    /// # Arguments
    /// * `input` - Input frame stream
    ///
    /// # Returns
    /// Stream of `EdgeSpectra`
    pub fn into_stream(
        mut self,
        input: impl Stream<Item = Frame> + Send + 'static,
    ) -> impl Stream<Item = EdgeSpectra> + Send + 'static {
        input.map(move |frame| self.process_frame(frame))
    }
}

impl Default for PrysmProcessor {
    fn default() -> Self {
        Self::new(&Config::default())
    }
}
