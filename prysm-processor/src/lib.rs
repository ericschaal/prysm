use futures::{Stream, StreamExt};
use nodes::{BandDetector, EdgeSampler, PixelDecoder, TemporalSmoothing};
use pipeline::Node;
use prysm_capture::Frame;
use prysm_core::{Config, EdgeSpectra};

mod frames;
mod nodes;
mod pipeline;

/// Processor using typed pipeline architecture
#[derive(Debug)]
pub struct PrysmProcessor {
    // Pipeline nodes (in order)
    decoder: PixelDecoder,
    band_detector: Option<BandDetector>,
    sampler: EdgeSampler,
    temporal_smoothing: Option<TemporalSmoothing>,
}

impl PrysmProcessor {
    pub fn new(config: &Config) -> Self {
        Self {
            decoder: PixelDecoder::new(),
            band_detector: if config.black_band_detection {
                Some(BandDetector::new(config))
            } else {
                None
            },
            sampler: EdgeSampler::new(
                config.sample_step,
                config.sample_density,
                config.edge_depth_px,
            ),
            temporal_smoothing: Some(TemporalSmoothing::new(config.temporal_smoothing)),
        }
    }

    /// Process a single frame through the pipeline
    pub fn process_frame(&mut self, frame: Frame) -> EdgeSpectra {
        let mut color_frame = self.decoder.process(frame);

        if let Some(ref mut detector) = self.band_detector {
            color_frame = detector.process(color_frame);
        }

        let mut spectra = self.sampler.process(color_frame);

        if let Some(ref mut smoother) = self.temporal_smoothing {
            spectra = smoother.process(spectra);
        }

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
