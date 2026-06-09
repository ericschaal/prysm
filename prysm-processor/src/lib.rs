use frames::ViewFrame;
use futures::{Stream, StreamExt};
use nodes::{BandDetector, ChangeDetector, EdgeSampler, TemporalSmoothing};
use pipeline::Node;
use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Config, EdgeSpectra};

mod frames;
mod nodes;
mod pipeline;

/// Processor using typed pipeline architecture.
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
    /// Output of the last processed frame, re-emitted when a frame is skipped
    last_spectra: Option<EdgeSpectra>,
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
            temporal_smoothing: Some(TemporalSmoothing::new(config.temporal_smoothing)),
            last_spectra: None,
        }
    }

    /// Process a single frame through the pipeline
    pub fn process_frame(&mut self, frame: Frame) -> EdgeSpectra {
        if !matches!(frame.format, PixelFormat::YUYV | PixelFormat::RGB24) {
            tracing::error!("{} format not yet supported", frame.format);
            return EdgeSpectra::black(
                frame.width as usize,
                frame.height as usize,
                self.config.sample_density,
            );
        }

        // Skip identical frames: re-emit the previous output untouched
        if let Some(ref mut detector) = self.change_detector
            && let Some(ref last) = self.last_spectra
            && !detector.has_changed(&frame)
        {
            return last.clone();
        }

        let mut view = ViewFrame::new(frame);

        if let Some(ref mut detector) = self.band_detector {
            view = detector.process(view);
        }

        let mut spectra = self.sampler.process(view);

        if let Some(ref mut smoother) = self.temporal_smoothing {
            spectra = smoother.process(spectra);
        }

        self.last_spectra = Some(spectra.clone());
        spectra
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
