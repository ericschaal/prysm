use crate::Stage;
use crate::frames::{FrameView, Viewport};
use crate::nodes::{BlackBarDetector, SpatialSampler, TemporalSmoother};
use crate::workers::Workers;
use prysm_core::{Config, EdgeColors, Frame, PixelFormat};
use std::num::NonZeroUsize;
use std::time::Instant;

/// Stateful frame processor.
///
/// Detection keeps the raw capture format; sampling reads RGB, YUYV and NV12 in
/// place, without a full-frame conversion, before reducing the picture to a grid
/// and projecting onto the perimeter.
/// Colour focus, when set, then lets each sample keep the hue next to it.
/// Pictures above the 640×360 pixel budget are sampled on a lattice;
/// grid dimensions and edge sample counts use the scaled picture size.
///
/// Cell averaging and colour focus may run on a small dedicated thread pool
/// owned by the processor (see [`Self::with_threads`]); each call still
/// returns only when its frame is done, with the same colours on any number
/// of threads.
#[derive(Debug)]
pub struct PrysmProcessor {
    pipeline: Pipeline,
    layout: Option<FrameLayout>,
    last_viewport: Option<Viewport>,
    workers: Workers,
}

/// Settings and nodes in processing order, configured together.
#[derive(Debug)]
struct Pipeline {
    config: Config,
    detector: Option<BlackBarDetector>,
    sampler: SpatialSampler,
    smoother: Option<TemporalSmoother>,
    /// Final brightness multiplier; absent when all output adjustments are neutral.
    output_gain: Option<f32>,
}

/// A layout change restarts black-bar detection and temporal history.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FrameLayout {
    width: u32,
    height: u32,
    format: PixelFormat,
}

impl FrameLayout {
    /// Whether frames of this layout can be sampled: empty frames cannot.
    fn is_sampled(self) -> bool {
        self.width > 0 && self.height > 0
    }
}

impl PrysmProcessor {
    /// A processor that samples on half the machine's cores, at most three
    /// threads, so the rest of the system keeps the other half; below four
    /// cores it uses the calling thread alone.
    pub fn new(config: &Config) -> Self {
        Self::with_threads(config, Workers::automatic_threads())
    }

    /// A processor that samples on `threads` dedicated pool threads, never
    /// rayon's global pool. With one thread it creates no pool and runs on
    /// the calling thread. The pool's threads end when the processor is dropped.
    pub fn with_threads(config: &Config, threads: NonZeroUsize) -> Self {
        let mut processor = Self {
            pipeline: Pipeline {
                config: *config,
                detector: None,
                sampler: SpatialSampler::default(),
                smoother: None,
                output_gain: None,
            },
            layout: None,
            last_viewport: None,
            workers: Workers::new(threads),
        };
        processor.configure(config);
        processor
    }

    /// Confirmed sampling area of the last supported frame, in source pixels.
    #[must_use]
    pub fn viewport(&self) -> Option<Viewport> {
        self.last_viewport
    }

    /// Apply processing settings without discarding unrelated temporal history.
    /// Call this when configuration changes, before processing the next frame.
    pub fn configure(&mut self, config: &Config) {
        let pipeline = &mut self.pipeline;
        pipeline.detector = config
            .remove_black_bars
            .then(|| pipeline.detector.take().unwrap_or_default());
        pipeline.smoother = match pipeline.smoother.take() {
            Some(smoother) => smoother.reconfigured(config),
            None => TemporalSmoother::new(config),
        };
        let gain = config.brightness_percent.as_ratio();
        pipeline.output_gain = (!config.saturation_boost_percent.is_neutral()
            || !config.dark_floor_percent.is_neutral()
            || !config.correction.is_identity()
            || gain < 1.0)
            .then_some(gain);
        pipeline.config = *config;
    }

    /// Process a single frame using the current monotonic time for smoothing.
    #[cfg(feature = "testkit")]
    pub fn process_frame(&mut self, frame: Frame) -> EdgeColors {
        self.process_frame_at(frame, Instant::now())
    }

    /// Process a frame at an explicit monotonic time, for deterministic replay.
    /// Both black-bar confirmation and smoothing use this clock.
    /// Use a shared clock origin plus source timestamps when replaying video.
    /// Repeated or earlier times do not advance smoothing.
    pub fn process_frame_at(&mut self, frame: Frame, now: Instant) -> EdgeColors {
        self.process_frame_staged(frame, now, |_| {})
    }

    /// [`Self::process_frame_at`], calling `mark` as each stage that ran
    /// ends, in pipeline order: a timing hook for the processor benchmark.
    #[doc(hidden)]
    pub fn process_frame_staged(
        &mut self,
        frame: Frame,
        now: Instant,
        mut mark: impl FnMut(Stage),
    ) -> EdgeColors {
        let layout = FrameLayout {
            width: frame.width(),
            height: frame.height(),
            format: frame.format(),
        };
        if self.layout != Some(layout) {
            self.reset();
            self.last_viewport = None;
            self.layout = Some(layout);
            if !layout.is_sampled() {
                tracing::error!(
                    target: "prysm_processor",
                    "Empty frames, shown as black: {} {}x{}",
                    layout.format,
                    layout.width,
                    layout.height
                );
            }
        }

        let mut edge_colors = if layout.is_sampled() {
            self.sample(frame, now, &mut mark)
        } else {
            EdgeColors::black(
                layout.width,
                layout.height,
                self.pipeline.config.sample_density,
            )
        };

        self.adjust_output(&mut edge_colors, mark);
        edge_colors
    }

    /// The stages before the output: black bars, sampling and smoothing.
    fn sample(&mut self, frame: Frame, now: Instant, mut mark: impl FnMut(Stage)) -> EdgeColors {
        let pipeline = &mut self.pipeline;
        let mut view = FrameView::new(frame);
        if let Some(detector) = &mut pipeline.detector {
            view = detector.process(view, now);
            mark(Stage::BlackBars);
        }
        self.last_viewport = Some(view.viewport);
        let mut edge_colors =
            pipeline
                .sampler
                .process_with(&view, &pipeline.config, &self.workers, &mut mark);
        if let Some(smoother) = &mut pipeline.smoother {
            edge_colors = smoother.process(edge_colors, now);
            mark(Stage::Smoothing);
        }
        edge_colors
    }

    /// Forgets black-bar evidence and temporal history, for a new frame
    /// layout. The sampler's geometry follows the picture size by itself.
    fn reset(&mut self) {
        if let Some(detector) = &mut self.pipeline.detector {
            *detector = BlackBarDetector::default();
        }
        if let Some(smoother) = &mut self.pipeline.smoother {
            smoother.reset();
        }
    }

    /// Boost chroma, apply the dark floor and calibration, then scale brightness.
    fn adjust_output(&self, edges: &mut EdgeColors, mut mark: impl FnMut(Stage)) {
        let Some(gain) = self.pipeline.output_gain else {
            return;
        };
        let config = &self.pipeline.config;
        for strip in [
            &mut edges.top,
            &mut edges.right,
            &mut edges.bottom,
            &mut edges.left,
        ] {
            strip.map_colors(|color| {
                let color = color
                    .boost_saturation(config.saturation_boost_percent)
                    .with_dark_floor(config.dark_floor_percent);
                config.correction.apply(color) * gain
            });
        }
        mark(Stage::Output);
    }
}

impl Default for PrysmProcessor {
    fn default() -> Self {
        Self::new(&Config::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{
        BrightnessPercent, ColorCorrection, DarkFloorPercent, SaturationBoostPercent,
        SmoothingSeconds,
    };

    /// What a node holds, as the table below names it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Node {
        Off,
        Fresh,
        Learned,
    }

    #[derive(Debug, Clone, Copy)]
    enum Event {
        /// Configure with the same switches and another brightness.
        Other,
        ToggleDetector,
        ToggleSmoother,
        Reset,
    }

    fn config(detector: bool, smoother: bool) -> Config {
        Config {
            remove_black_bars: detector,
            smoothing_seconds: SmoothingSeconds(if smoother { 0.1 } else { 0.0 }),
            ..Config::default()
        }
    }

    fn detector(pipeline: &Pipeline) -> Node {
        let fresh = format!("{:?}", BlackBarDetector::default());
        pipeline.detector.as_ref().map_or(Node::Off, |detector| {
            if format!("{detector:?}") == fresh {
                Node::Fresh
            } else {
                Node::Learned
            }
        })
    }

    fn smoother(pipeline: &Pipeline, config: &Config) -> Node {
        pipeline.smoother.as_ref().map_or(Node::Off, |smoother| {
            let fresh = TemporalSmoother::new(config).expect("smoothing is on");
            if format!("{smoother:?}") == format!("{fresh:?}") {
                Node::Fresh
            } else {
                Node::Learned
            }
        })
    }

    /// A processor in the given states: a node is taught by one frame.
    fn processor(detector: Node, smoother: Node) -> (PrysmProcessor, Config) {
        let config = config(detector != Node::Off, smoother != Node::Off);
        let mut processor = PrysmProcessor::with_threads(&config, NonZeroUsize::MIN);
        let frame = crate::frames::FrameView::new(prysm_core::Frame::dummy(64, 36).unwrap());
        if let (Node::Learned, Some(node)) = (detector, &mut processor.pipeline.detector) {
            let _ = node.process(frame, Instant::now());
        }
        if let (Node::Learned, Some(node)) = (smoother, &mut processor.pipeline.smoother) {
            let _ = node.process(EdgeColors::default(), Instant::now());
        }
        (processor, config)
    }

    #[test]
    fn configure_keeps_what_a_node_has_learned_unless_it_is_switched_and_reset_forgets_it() {
        use Event::{Other, Reset, ToggleDetector, ToggleSmoother};
        use Node::{Fresh, Learned, Off};
        // (detector, smoother), event, then (detector, smoother).
        let table = [
            ((Off, Off), Other, (Off, Off)),
            ((Off, Off), ToggleDetector, (Fresh, Off)),
            ((Off, Off), ToggleSmoother, (Off, Fresh)),
            ((Off, Off), Reset, (Off, Off)),
            ((Fresh, Fresh), Other, (Fresh, Fresh)),
            ((Fresh, Fresh), ToggleDetector, (Off, Fresh)),
            ((Fresh, Fresh), ToggleSmoother, (Fresh, Off)),
            ((Fresh, Fresh), Reset, (Fresh, Fresh)),
            ((Learned, Learned), Other, (Learned, Learned)),
            ((Learned, Learned), ToggleDetector, (Off, Learned)),
            ((Learned, Learned), ToggleSmoother, (Learned, Off)),
            ((Learned, Learned), Reset, (Fresh, Fresh)),
            ((Learned, Off), ToggleSmoother, (Learned, Fresh)),
            ((Off, Learned), ToggleDetector, (Fresh, Learned)),
        ];
        for ((before_detector, before_smoother), event, expected) in table {
            let (mut processor, mut config) = processor(before_detector, before_smoother);
            match event {
                Other => config.brightness_percent = BrightnessPercent(40),
                ToggleDetector => config.remove_black_bars = !config.remove_black_bars,
                ToggleSmoother => {
                    let on = config.smoothing_seconds.is_enabled();
                    config.smoothing_seconds = SmoothingSeconds(if on { 0.0 } else { 0.1 });
                }
                Reset => {}
            }
            match event {
                Reset => processor.reset(),
                _ => processor.configure(&config),
            }
            let pipeline = &processor.pipeline;
            assert_eq!(
                (detector(pipeline), smoother(pipeline, &config)),
                expected,
                "{before_detector:?}, {before_smoother:?} after {event:?}"
            );
        }
    }

    #[test]
    fn the_output_stage_exists_only_when_a_step_is_not_neutral() {
        let mut processor = PrysmProcessor::with_threads(&Config::default(), NonZeroUsize::MIN);
        assert!(processor.pipeline.output_gain.is_none());
        for brightness in [100, 101, 255] {
            let config = Config {
                brightness_percent: BrightnessPercent(brightness),
                ..Config::default()
            };
            processor.configure(&config);
            assert!(processor.pipeline.output_gain.is_none(), "{brightness}");
        }
        for config in [
            Config {
                brightness_percent: BrightnessPercent(99),
                ..Config::default()
            },
            Config {
                saturation_boost_percent: SaturationBoostPercent(1),
                ..Config::default()
            },
            Config {
                dark_floor_percent: DarkFloorPercent(1),
                ..Config::default()
            },
            Config {
                correction: ColorCorrection::new(0, 0, 10, Default::default()).unwrap(),
                ..Config::default()
            },
        ] {
            processor.configure(&config);
            assert!(processor.pipeline.output_gain.is_some(), "{config:?}");
        }
    }
}
