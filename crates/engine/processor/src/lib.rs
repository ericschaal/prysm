#![forbid(unsafe_code)]
mod frames;
mod nodes;
mod pipeline;
mod workers;

pub use frames::{SampleError, sample_rgb};
pub use nodes::cut_strength;
pub use pipeline::PrysmProcessor;

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

#[cfg(test)]
mod thread_tests {
    use super::*;
    use prysm_core::{
        ColorFocusPercent, Config, Frame, PixelFormat, SampleDensity, SpatialReachPercent,
        YuvRange, YuvStandardMatrix,
    };
    use std::{num::NonZeroUsize, time::Instant};

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
    use prysm_core::{ColorCorrection, Config, Frame, LinearColor, PixelFormat};
    use std::time::Instant;

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
        config.correction =
            prysm_core::ColorCorrection::new(60, -30, 40, Default::default()).unwrap();
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
        let warm = ColorCorrection::new(60, -30, 40, Default::default()).unwrap();
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
    fn empty_frames_get_the_output_stage_of_a_black_picture() {
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
            Frame::fill(0, 0, 2, PixelFormat::RGB24).unwrap(),
            Frame::fill(0, 4, 0, PixelFormat::RGB24).unwrap(),
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
