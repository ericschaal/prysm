//! The pipeline's nodes and the one mapping of [`Config`] onto them.
use crate::nodes::{BlackBarDetector, SpatialSampler, TemporalSmoother};
use prysm_core::{ColorCorrection, Config, DarkFloorPercent, LinearColor, SaturationBoostPercent};

/// The nodes, in pipeline order. [`Self::configure`] is the only place that
/// turns settings into nodes; the sampler reads its settings from the
/// [`Config`] it is given with each frame.
#[derive(Debug)]
pub(crate) struct Pipeline {
    pub(crate) detector: Option<BlackBarDetector>,
    pub(crate) sampler: SpatialSampler,
    pub(crate) smoother: Option<TemporalSmoother>,
    pub(crate) output: Option<OutputStage>,
}

impl Pipeline {
    pub(crate) fn new(config: &Config) -> Self {
        let mut pipeline = Self {
            detector: None,
            sampler: SpatialSampler::default(),
            smoother: None,
            output: None,
        };
        pipeline.configure(config);
        pipeline
    }

    /// Applies `config`. A node that stays on keeps what it has learned:
    /// black-bar evidence and crops, and temporal history. A node switched
    /// on starts afresh; one switched off is dropped.
    pub(crate) fn configure(&mut self, config: &Config) {
        self.detector = config
            .remove_black_bars
            .then(|| self.detector.take().unwrap_or_default());
        self.smoother = match self.smoother.take() {
            Some(smoother) => smoother.reconfigured(config),
            None => TemporalSmoother::new(config),
        };
        self.output = OutputStage::new(config);
    }

    /// Forgets black-bar evidence and temporal history, for a new frame
    /// layout. The sampler's geometry follows the picture size by itself.
    pub(crate) fn reset(&mut self) {
        if let Some(detector) = &mut self.detector {
            *detector = BlackBarDetector::default();
        }
        if let Some(smoother) = &mut self.smoother {
            smoother.reset();
        }
    }
}

/// Each light's last steps: colour boost, dark floor, calibration and
/// brightness. Every step is exact identity at its neutral setting, and the
/// stage exists only when some step is not neutral.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputStage {
    boost: SaturationBoostPercent,
    floor: DarkFloorPercent,
    correction: ColorCorrection,
    gain: f32,
}

impl OutputStage {
    /// `None` when every step is neutral.
    fn new(config: &Config) -> Option<Self> {
        let stage = Self {
            boost: config.saturation_boost_percent,
            floor: config.dark_floor_percent,
            correction: config.correction,
            gain: config.brightness_percent.as_ratio(),
        };
        (!stage.boost.is_neutral()
            || !stage.floor.is_neutral()
            || !stage.correction.is_identity()
            || stage.gain < 1.0)
            .then_some(stage)
    }

    pub(crate) fn apply(&self, color: LinearColor) -> LinearColor {
        let color = color
            .boost_saturation(self.boost)
            .with_dark_floor(self.floor);
        // The glow is meant as neutral white on the wall, so the calibration
        // corrects it and brightness scales it like any other colour.
        self.correction.apply(color) * self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{BrightnessPercent, EdgeColors, SmoothingSeconds};
    use std::time::Instant;

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

    /// A pipeline in the given states: a node is taught by one frame.
    fn pipeline(detector: Node, smoother: Node) -> (Pipeline, Config) {
        let config = config(detector != Node::Off, smoother != Node::Off);
        let mut pipeline = Pipeline::new(&config);
        let frame = crate::frames::FrameView::new(prysm_core::Frame::dummy(64, 36).unwrap());
        if let (Node::Learned, Some(node)) = (detector, &mut pipeline.detector) {
            let _ = node.process(frame, Instant::now());
        }
        if let (Node::Learned, Some(node)) = (smoother, &mut pipeline.smoother) {
            let _ = node.process(EdgeColors::default(), Instant::now());
        }
        (pipeline, config)
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
            let (mut pipeline, mut config) = pipeline(before_detector, before_smoother);
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
                Reset => pipeline.reset(),
                _ => pipeline.configure(&config),
            }
            assert_eq!(
                (detector(&pipeline), smoother(&pipeline, &config)),
                expected,
                "{before_detector:?}, {before_smoother:?} after {event:?}"
            );
        }
    }

    #[test]
    fn the_output_stage_exists_only_when_a_step_is_not_neutral() {
        assert!(OutputStage::new(&Config::default()).is_none());
        for brightness in [100, 101, 255] {
            let config = Config {
                brightness_percent: BrightnessPercent(brightness),
                ..Config::default()
            };
            assert!(OutputStage::new(&config).is_none(), "{brightness}");
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
                correction: ColorCorrection::new(0, 0, 10).unwrap(),
                ..Config::default()
            },
        ] {
            assert!(OutputStage::new(&config).is_some(), "{config:?}");
        }
    }
}
