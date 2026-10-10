use frames::{ViewFrame, Viewport};
use futures::{Stream, StreamExt};
use nodes::{BandDetector, ChangeDetector, EdgeSampler, TemporalSmoothing};
use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Config, EdgeSpectra};

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
    change_detector: Option<ChangeDetector>,
    band_detector: Option<BandDetector>,
    sampler: EdgeSampler,
    temporal_smoothing: Option<TemporalSmoothing>,
    /// Last sampled target, before temporal smoothing
    last_spectra: Option<EdgeSpectra>,
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
            change_detector: if config.change_detection {
                Some(ChangeDetector::new(config))
            } else {
                None
            },
            band_detector: if config.black_band_detection {
                Some(BandDetector::new(config))
            } else {
                None
            },
            sampler: EdgeSampler::new(config.sample_density, config.edge_depth),
            temporal_smoothing: (config.temporal_smoothing > 0.0)
                .then(|| TemporalSmoothing::new(config.temporal_smoothing)),
            last_spectra: None,
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
                self.config.sample_density,
            );
        }

        let changed = self
            .change_detector
            .as_mut()
            .is_none_or(|detector| detector.has_changed(&frame));
        let mut view = ViewFrame::new(frame);

        if let Some(ref mut detector) = self.band_detector {
            view = detector.process(view);
        }

        if self.last_viewport != Some(view.viewport)
            && let Some(smoother) = &mut self.temporal_smoothing
        {
            // Old samples describe a different region of the image.
            *smoother = TemporalSmoothing::new(self.config.temporal_smoothing);
        }

        // Crop confirmation and smoothing still advance on unchanged frames.
        let mut spectra = match &self.last_spectra {
            Some(target) if !changed && self.last_viewport == Some(view.viewport) => target.clone(),
            _ => {
                self.last_viewport = Some(view.viewport);
                let target = self.sampler.process(&view);
                self.last_spectra = Some(target.clone());
                target
            }
        };

        if let Some(ref mut smoother) = self.temporal_smoothing {
            spectra = smoother.process(spectra);
        }

        spectra * self.config.brightness.clamp(0.0, 1.0)
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
