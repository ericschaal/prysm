use frames::{FrameView, Viewport};
use nodes::{BlackBarDetector, SpatialSampler, TemporalSmoother};
use prysm_core::{ColorCorrection, Config, EdgeColors, Frame, LinearColor, PixelFormat};
use std::num::NonZeroUsize;
use std::time::Instant;
use workers::Workers;

mod frames;
mod nodes;
mod workers;

pub use nodes::cut_strength;

/// Stateful frame processor.
///
/// Detection keeps the raw capture format; sampling reads RGB, YUYV and NV12 in
/// place, without a full-frame conversion, before reducing the picture to a grid
/// and projecting onto the perimeter.
/// Colour focus, when set, then lets each sample keep the hue next to it.
/// Pictures above 640x360 are averaged on a lattice of every nth pixel and row
/// and get the grid of 640x360, so sampling costs about the same at any capture size.
///
/// Cell averaging and colour focus may run on a small dedicated thread pool
/// owned by the processor (see [`Self::with_threads`]); each call still
/// returns only when its frame is done, with the same colours on any number
/// of threads.
#[derive(Debug)]
pub struct PrysmProcessor {
    config: Config,
    // Pipeline nodes (in order)
    black_bar_detector: Option<BlackBarDetector>,
    sampler: SpatialSampler,
    temporal_smoother: Option<TemporalSmoother>,
    last_viewport: Option<Viewport>,
    frame_layout: Option<FrameLayout>,
    workers: Workers,
}

/// A pipeline stage, as [`PrysmProcessor::process_frame_staged`] reports it.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Black-bar detection, with bars removed.
    BlackBars,
    /// Each grid cell's mean linear colour: pixel decoding, linearising, sums.
    CellAverages,
    /// Each light's plain weighted mean of the cells.
    Projection,
    /// Colour focus, with focus above 0.
    ColorFocus,
    /// Temporal smoothing, with smoothing on.
    Smoothing,
    /// Colour boost, dark floor, calibration and brightness, when any is not neutral.
    Output,
}

/// A layout change invalidates geometry and temporal history.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FrameLayout {
    width: u32,
    height: u32,
    format: PixelFormat,
}

impl FrameLayout {
    /// Whether frames of this layout can be sampled: compressed or empty
    /// frames cannot.
    fn is_sampled(self) -> bool {
        self.width > 0 && self.height > 0 && self.format != PixelFormat::MJPEG
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
        Self::with_workers(config, Workers::new(threads))
    }

    fn with_workers(config: &Config, workers: Workers) -> Self {
        let mut sampler = SpatialSampler::new(
            config.sample_density,
            config.spatial_reach_percent.as_ratio(),
        );
        sampler.set_color_focus(config.color_focus_percent);
        Self {
            config: *config,
            black_bar_detector: config.remove_black_bars.then(BlackBarDetector::new),
            sampler,
            temporal_smoother: TemporalSmoother::new(config),
            last_viewport: None,
            frame_layout: None,
            workers,
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
        self.sampler.set_color_focus(config.color_focus_percent);
        if config.remove_black_bars != self.config.remove_black_bars {
            self.black_bar_detector = config.remove_black_bars.then(BlackBarDetector::new);
        }
        self.temporal_smoother = match self.temporal_smoother.take() {
            Some(smoother) => smoother.reconfigured(config),
            None => TemporalSmoother::new(config),
        };
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
        if self.frame_layout != Some(layout) {
            // A new layout starts afresh, on the same threads.
            let workers = std::mem::take(&mut self.workers);
            *self = Self::with_workers(&self.config, workers);
            self.frame_layout = Some(layout);
            if !layout.is_sampled() {
                tracing::error!(
                    "Unsupported frames, shown as black: {} {}x{}",
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
                layout.width as usize,
                layout.height as usize,
                self.config.sample_density,
            )
        };

        let config = &self.config;
        // Every output step is exact identity at its neutral setting.
        if config.correction != ColorCorrection::default()
            || config.saturation_boost_percent.0 != 0
            || config.dark_floor_percent.0 != 0
            || config.brightness_percent.0 < 100
        {
            for strip in [
                &mut edge_colors.top,
                &mut edge_colors.right,
                &mut edge_colors.bottom,
                &mut edge_colors.left,
            ] {
                strip.map_colors(|color| output_color(config, color));
            }
            mark(Stage::Output);
        }
        edge_colors
    }

    /// The stages before the output: black bars, sampling and smoothing.
    fn sample(&mut self, frame: Frame, now: Instant, mut mark: impl FnMut(Stage)) -> EdgeColors {
        let mut view = FrameView::new(frame);

        if let Some(ref mut detector) = self.black_bar_detector {
            view = detector.process(view, now);
            mark(Stage::BlackBars);
        }

        self.last_viewport = Some(view.viewport);
        let mut edge_colors = self.sampler.process_with(&view, &self.workers, &mut mark);

        if let Some(ref mut smoother) = self.temporal_smoother {
            edge_colors = smoother.process(edge_colors, now);
            mark(Stage::Smoothing);
        }
        edge_colors
    }
}

/// One light's output stage: colour boost, dark floor, calibration, brightness.
fn output_color(config: &Config, color: LinearColor) -> LinearColor {
    let color = color
        .boost_saturation(config.saturation_boost_percent)
        .with_dark_floor(config.dark_floor_percent);
    // The glow is meant as neutral white on the wall, so the calibration
    // corrects it and brightness scales it like any other colour.
    config.correction.apply(color) * config.brightness_percent.as_ratio()
}

impl Default for PrysmProcessor {
    fn default() -> Self {
        Self::new(&Config::default())
    }
}

#[cfg(test)]
mod thread_tests {
    use super::*;
    use prysm_core::{
        ColorFocusPercent, SampleDensity, SpatialReachPercent, YuvRange, YuvStandardMatrix,
    };

    /// A letterboxed NV12 picture of gradients and noise that moves with `seed`.
    fn nv12(width: u32, height: u32, seed: u32) -> Frame {
        let (w, h) = (width as usize, height as usize);
        let mut state = 0x9E37_79B9_u32 ^ seed.wrapping_mul(0x85EB_CA6B);
        let mut noise = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state.to_le_bytes()[0] & 31
        };
        let bar = h / 8;
        let mut data = vec![128_u8; w * h * 3 / 2];
        for y in 0..h {
            for x in 0..w {
                data[y * w + x] = if y < bar || y >= h - bar {
                    16
                } else {
                    let level = (x * 160 / w + y * 40 / h + seed as usize) % 200;
                    16 + u8::try_from(level).unwrap() + noise()
                };
            }
        }
        for (i, pair) in data[w * h..].as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let (x, y) = (i % (w / 2), i / (w / 2));
            pair[0] = 16 + u8::try_from((x * 7 + seed as usize) % 200).unwrap() + noise() / 2;
            pair[1] = 16 + u8::try_from((y * 5 + x) % 200).unwrap() + noise() / 2;
        }
        let format = PixelFormat::NV12 {
            range: YuvRange::Limited,
            matrix: YuvStandardMatrix::Bt709,
        };
        Frame::new(data, width, height, format).unwrap()
    }

    #[test]
    fn three_threads_give_bit_identical_colours_to_one() {
        let base = Config {
            saturation_boost_percent: prysm_core::SaturationBoostPercent(30),
            dark_floor_percent: prysm_core::DarkFloorPercent(3),
            ..Config::default()
        };
        let configs = [
            base,
            Config {
                color_focus_percent: ColorFocusPercent(10),
                ..base
            },
            Config {
                color_focus_percent: ColorFocusPercent(100),
                spatial_reach_percent: SpatialReachPercent(100),
                ..base
            },
            Config {
                color_focus_percent: ColorFocusPercent(60),
                sample_density: SampleDensity(2000),
                remove_black_bars: false,
                ..base
            },
        ];
        let one = NonZeroUsize::MIN;
        let three = NonZeroUsize::new(3).unwrap();
        for config in configs {
            for (width, height) in [(640, 360), (1280, 720), (96, 54)] {
                let mut serial = PrysmProcessor::with_threads(&config, one);
                let mut parallel = PrysmProcessor::with_threads(&config, three);
                let start = Instant::now();
                for seed in 0..6 {
                    let frame = nv12(width, height, seed * 37);
                    let now = start + std::time::Duration::from_millis(u64::from(seed) * 33);
                    assert_eq!(
                        parallel.process_frame_at(frame.clone(), now),
                        serial.process_frame_at(frame, now),
                        "{width}x{height} frame {seed}: {config:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn stages_are_reported_in_pipeline_order() {
        let config = Config {
            color_focus_percent: ColorFocusPercent(50),
            brightness_percent: prysm_core::BrightnessPercent(80),
            ..Config::default()
        };
        let mut processor = PrysmProcessor::with_threads(&config, NonZeroUsize::MIN);
        let mut stages = Vec::new();
        let _ = processor.process_frame_staged(nv12(64, 36, 1), Instant::now(), |stage| {
            stages.push(stage);
        });
        assert_eq!(
            stages,
            [
                Stage::BlackBars,
                Stage::CellAverages,
                Stage::Projection,
                Stage::ColorFocus,
                Stage::Smoothing,
                Stage::Output
            ]
        );
    }
}

#[cfg(test)]
mod correction_tests {
    use super::*;
    #[test]
    fn live_calibration_changes_precede_brightness_and_do_not_retain_previous_correction() {
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

    #[test]
    fn the_dark_floor_glow_is_calibrated_and_scaled_by_brightness() {
        let black = Frame::fill(0, 4, 2, PixelFormat::RGB24).unwrap();
        let glow = |correction| {
            let config = Config {
                remove_black_bars: false,
                smoothing_seconds: prysm_core::SmoothingSeconds(0.0),
                brightness_percent: prysm_core::BrightnessPercent(50),
                dark_floor_percent: prysm_core::DarkFloorPercent(10),
                correction,
                ..Config::default()
            };
            PrysmProcessor::new(&config)
                .process_frame(black.clone())
                .top
                .sample_at(0.5)
        };
        let neutral = glow(ColorCorrection::default());
        for channel in [neutral.r, neutral.g, neutral.b] {
            assert!((channel - 0.05).abs() < 1e-6, "{neutral:?}");
        }
        let warm = ColorCorrection::new(60, -30, 40).unwrap();
        let corrected = glow(warm);
        let expected = warm.apply(LinearColor::new(0.1, 0.1, 0.1)) * 0.5;
        for (actual, expected) in [
            (corrected.r, expected.r),
            (corrected.g, expected.g),
            (corrected.b, expected.b),
        ] {
            assert!((actual - expected).abs() < 1e-6, "{corrected:?}");
        }
        assert!(
            corrected.r > corrected.b,
            "the warm calibration reaches the glow"
        );
    }

    #[test]
    fn unsupported_frames_get_the_output_stage_of_a_black_picture() {
        let config = Config {
            remove_black_bars: false,
            smoothing_seconds: prysm_core::SmoothingSeconds(0.0),
            brightness_percent: prysm_core::BrightnessPercent(50),
            dark_floor_percent: prysm_core::DarkFloorPercent(10),
            ..Config::default()
        };
        let black = PrysmProcessor::new(&config)
            .process_frame(Frame::fill(0, 4, 2, PixelFormat::RGB24).unwrap())
            .top
            .sample_at(0.5);
        for frame in [
            Frame::new(vec![], 4, 2, PixelFormat::MJPEG).unwrap(),
            Frame::fill(0, 0, 2, PixelFormat::RGB24).unwrap(),
        ] {
            let mut processor = PrysmProcessor::new(&config);
            let mut stages = Vec::new();
            let output = processor.process_frame_staged(frame, Instant::now(), |stage| {
                stages.push(stage);
            });
            assert_eq!(output.top.sample_at(0.5), black);
            assert_eq!(stages, [Stage::Output]);
        }
    }
}
