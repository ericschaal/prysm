use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Config, SampleDensity};
use prysm_processor::PrysmProcessor;
use std::time::{Duration, Instant};

fn rgb(width: u32, height: u32, value: u8) -> Frame {
    Frame::fill(value, width, height, PixelFormat::RGB24)
}

#[test]
fn wide_black_bands_crop_each_axis_independently() {
    let config = Config {
        brightness_percent: 100,
        smoothing_seconds: 0.0,
        sample_density: SampleDensity(150),
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    for (x_start, x_end, y_start, y_end) in [(219, 421, 0, 360), (0, 640, 130, 230)] {
        let mut data = vec![0; 640 * 360 * 3];
        for y in y_start..y_end {
            data[(y * 640 + x_start) * 3..(y * 640 + x_end) * 3].fill(255);
        }
        let frame = Frame::new(data, 640, 360, PixelFormat::RGB24);
        for _ in 0..120 {
            processor.process_frame(frame.clone());
        }
        let output = processor.process_frame(frame);
        assert_eq!(
            output.top.len(),
            config.sample_density.samples_for_length(x_end - x_start)
        );
        assert_eq!(
            output.left.len(),
            config.sample_density.samples_for_length(y_end - y_start)
        );
        for edge in [&output.top, &output.bottom, &output.left, &output.right] {
            assert!(edge.sample_at(0.5).r > 0.99);
        }
    }
}

#[test]
fn slightly_noisy_black_bands_still_crop() {
    use prysm_capture::YuvRange;
    let config = Config {
        brightness_percent: 100,
        smoothing_seconds: 0.0,
        ..Config::default()
    };
    for (range, black, white) in [(YuvRange::Full, 0, 255), (YuvRange::Limited, 16, 235)] {
        for horizontal in [true, false] {
            let mut data = vec![128; 640 * 360 * 2];
            for y in 0..360 {
                for x in 0..640 {
                    let (position, start, end) = if horizontal {
                        (y, 48, 312)
                    } else {
                        (x, 80, 560)
                    };
                    data[(y * 640 + x) * 2] = if (start..end).contains(&position) {
                        white
                    } else {
                        black + (position % 2) as u8
                    };
                }
            }
            let mut frame = Frame::new(data, 640, 360, PixelFormat::YUYV);
            frame.yuv_range = range;
            let mut processor = PrysmProcessor::new(&config);
            for _ in 0..60 {
                processor.process_frame(frame.clone());
            }
            let output = processor.process_frame(frame);
            let (width, height) = if horizontal { (640, 264) } else { (480, 360) };
            assert_eq!(
                output.top.len(),
                SampleDensity::default().samples_for_length(width)
            );
            assert_eq!(
                output.left.len(),
                SampleDensity::default().samples_for_length(height)
            );
            for edge in [&output.top, &output.bottom, &output.left, &output.right] {
                assert!(
                    edge.sample_at(0.5).r > 0.99,
                    "{range:?}, horizontal={horizontal}"
                );
            }
        }
    }
}

#[test]
fn edge_sampling_includes_the_last_row_and_column() {
    let config = Config {
        brightness_percent: 100,
        smoothing_seconds: 0.0,
        remove_black_bars: false,
        ..Config::default()
    };
    for (width, height) in [(636, 360), (360, 636)] {
        let mut data = vec![0; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                if (width == 636 && x == width - 1) || (height == 636 && y == height - 1) {
                    data[(y * width + x) * 3..(y * width + x + 1) * 3].fill(255);
                }
            }
        }
        let output = PrysmProcessor::new(&config).process_frame(Frame::new(
            data,
            width as u32,
            height as u32,
            PixelFormat::RGB24,
        ));
        let edges = if width == 636 {
            [&output.top, &output.bottom]
        } else {
            [&output.left, &output.right]
        };
        for edge in edges {
            // At density 60, the last segment of this 636px edge spans 17 pixels.
            assert!((edge.sample_at(1.0).r - 1.0 / 17.0).abs() < 0.00001);
        }
    }
}

#[test]
fn dark_frames_must_not_panic() {
    let mut p = PrysmProcessor::default();
    for _ in 0..1900 {
        let edge_colors = p.process_frame(rgb(64, 36, 20));
        assert!(edge_colors.top.sample_at(0.5).r > 0.0);
    }
}

#[test]
fn smoothing_must_keep_converging_on_static_input() {
    let c = Config {
        brightness_percent: 100,
        remove_black_bars: false,
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    let start = Instant::now();
    p.process_frame_at(rgb(64, 36, 255), start);
    let black = rgb(64, 36, 0);
    let first = p
        .process_frame_at(black.clone(), start + Duration::from_millis(10))
        .top
        .sample_at(0.5)
        .r;
    let next = p
        .process_frame_at(black, start + Duration::from_millis(20))
        .top
        .sample_at(0.5)
        .r;
    assert!(next < first, "smoothing stopped: {first} -> {next}");
}

#[test]
fn color_change_must_update_output() {
    let c = Config {
        brightness_percent: 100,
        remove_black_bars: false,
        smoothing_seconds: 0.0,
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    let a = Frame::new(
        [128, 128, 128, 128].repeat(64 * 36 / 2),
        64,
        36,
        PixelFormat::YUYV,
    );
    let b = Frame::new(
        [128, 128, 128, 200].repeat(64 * 36 / 2),
        64,
        36,
        PixelFormat::YUYV,
    );
    p.process_frame(a.clone());
    p.process_frame(a);
    let actual = p.process_frame(b.clone());
    let expected = PrysmProcessor::new(&c).process_frame(b);
    assert_eq!(actual, expected, "same luma, changed chroma was skipped");
}

#[test]
fn static_letterbox_confirms_with_default_tuning() {
    let c = Config {
        brightness_percent: 100,
        smoothing_seconds: 0.0,
        ..Config::default()
    };
    let mut data = vec![128; 640 * 360 * 3];
    data[..640 * 48 * 3].fill(0);
    data[640 * 312 * 3..].fill(0);
    let frame = Frame::new(data, 640, 360, PixelFormat::RGB24);
    let mut p = PrysmProcessor::new(&c);
    let mut actual = p.process_frame(frame.clone());
    for _ in 1..60 {
        actual = p.process_frame(frame.clone());
    }
    assert!(
        actual.top.sample_at(0.5).r > 0.1,
        "letterbox still unconfirmed after 60 frames"
    );
}

#[test]
fn resize_must_update_sample_count() {
    let c = Config {
        brightness_percent: 100,
        remove_black_bars: false,
        sample_density: SampleDensity(150),
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    p.process_frame(rgb(640, 360, 128));
    p.process_frame(rgb(640, 360, 128));
    let actual = p.process_frame(rgb(320, 180, 128));
    assert_eq!(actual.top.len(), c.sample_density.samples_for_length(320));
}

#[test]
fn bgr_supported_by_capture_must_process() {
    let c = Config {
        brightness_percent: 100,
        remove_black_bars: false,
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    let actual = p.process_frame(Frame::new(
        [0, 0, 255].repeat(64 * 36),
        64,
        36,
        PixelFormat::BGR24,
    ));
    let color = actual.top.sample_at(0.5);
    assert!(
        color.r > 0.99 && color.g == 0.0 && color.b == 0.0,
        "BGR red was decoded incorrectly: {color:?}"
    );
}

#[test]
fn dark_scene_preserves_confirmed_crop() {
    let c = Config {
        brightness_percent: 100,
        smoothing_seconds: 0.0,
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    let mut data = vec![128; 640 * 360 * 3];
    data[..640 * 48 * 3].fill(0);
    data[640 * 312 * 3..].fill(0);
    let letterbox = Frame::new(data, 640, 360, PixelFormat::RGB24);
    for _ in 0..60 {
        p.process_frame(letterbox.clone());
    }
    let cropped = p.process_frame(letterbox);
    assert_eq!(
        cropped.left.len(),
        SampleDensity::default().samples_for_length(264)
    );
    for _ in 0..60 {
        let dark = p.process_frame(rgb(640, 360, 20));
        assert_eq!(dark.left.len(), cropped.left.len());
    }
    let resized = p.process_frame(rgb(320, 180, 128));
    assert_eq!(
        resized,
        PrysmProcessor::new(&c).process_frame(rgb(320, 180, 128))
    );
}

#[test]
fn format_change_clears_smoothing_history() {
    let mut p = PrysmProcessor::default();
    p.process_frame(rgb(64, 36, 255));
    let dark = p.process_frame(Frame::fill(0, 64, 36, PixelFormat::BGR24));
    assert!(dark.top.sample_at(0.5).r.abs() < f32::EPSILON);
}

#[test]
fn empty_frames_return_black() {
    let mut p = PrysmProcessor::default();
    for (width, height) in [(0, 36), (64, 0)] {
        let edge_colors = p.process_frame(rgb(width, height, 128));
        assert!(edge_colors.top.sample_at(0.5).r.abs() < f32::EPSILON);
    }
}

#[test]
fn equal_mean_rgb_colors_update_output() {
    let c = Config {
        brightness_percent: 100,
        remove_black_bars: false,
        smoothing_seconds: 0.0,
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    let red = Frame::new([255, 0, 0].repeat(64 * 36), 64, 36, PixelFormat::RGB24);
    let blue = Frame::new([0, 0, 255].repeat(64 * 36), 64, 36, PixelFormat::RGB24);
    p.process_frame(red.clone());
    p.process_frame(red);
    let color = p.process_frame(blue).top.sample_at(0.5);
    assert!(
        color.r == 0.0 && color.b > 0.99,
        "color change was skipped: {color:?}"
    );
}

#[test]
fn brightness_scales_output_once_after_smoothing() {
    for brightness_percent in [0, 50, 100, 255] {
        let config = Config {
            brightness_percent,
            remove_black_bars: false,
            ..Config::default()
        };
        let mut processor = PrysmProcessor::new(&config);
        for _ in 0..3 {
            let color = processor.process_frame(rgb(64, 36, 255)).top.sample_at(0.5);
            assert!(
                (color.r - f32::from(brightness_percent.min(100)) / 100.0).abs() < 1e-6,
                "brightness {brightness_percent}%: {color:?}"
            );
        }
    }
}

#[test]
fn narrow_edge_changes_update_output_immediately() {
    let config = Config {
        smoothing_seconds: 0.0,
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    let base = rgb(640, 360, 128);
    let mut data = base.data.as_ref().clone();
    data[..640 * 10 * 3].fill(255);
    let changed = Frame::new(data, 640, 360, PixelFormat::RGB24);
    let old = processor.process_frame(base);
    let actual = processor.process_frame(changed.clone());
    let expected = PrysmProcessor::new(&config).process_frame(changed);
    assert_ne!(actual, old);
    assert_eq!(actual, expected);
}

#[test]
fn bright_objects_grow_stronger_as_they_approach_each_edge() {
    let config = Config {
        brightness_percent: 100,
        smoothing_seconds: 0.0,
        remove_black_bars: false,
        ..Config::default()
    };
    for (width, height) in [(320, 180), (640, 360)] {
        let depth = height / 50;
        for edge in 0..4 {
            let mut processor = PrysmProcessor::new(&config);
            let mut previous = 0.0;
            for inset_percent in [16, 14, 10, 5, 0] {
                let inset = height * inset_percent / 100;
                let (x_start, y_start, x_end, y_end) = match edge {
                    0 => (width / 3, inset, width * 2 / 3, inset + depth),
                    1 => (
                        width - inset - depth,
                        height / 3,
                        width - inset,
                        height * 2 / 3,
                    ),
                    2 => (
                        width / 3,
                        height - inset - depth,
                        width * 2 / 3,
                        height - inset,
                    ),
                    _ => (inset, height / 3, inset + depth, height * 2 / 3),
                };
                let mut data = vec![0; (width * height * 3) as usize];
                for y in y_start..y_end {
                    for x in x_start..x_end {
                        data[((y * width + x) * 3) as usize] = 255;
                    }
                }
                let output =
                    processor.process_frame(Frame::new(data, width, height, PixelFormat::RGB24));
                let edges = [&output.top, &output.right, &output.bottom, &output.left];
                let actual = edges[edge].sample_at(0.5);
                if inset_percent == 16 {
                    assert_eq!(actual.r, 0.0);
                } else {
                    assert!(
                        actual.r > previous,
                        "{width}x{height}, edge {edge}, inset {inset_percent}%: {actual:?} <= {previous}"
                    );
                }
                assert!(
                    actual.r < 0.5,
                    "a small bright object must not turn the whole sample fully on"
                );
                assert_eq!((actual.g, actual.b), (0.0, 0.0));
                assert_eq!(edges[(edge + 2) % 4].sample_at(0.5).r, 0.0);
                previous = actual.r;
            }
        }
    }
}

#[test]
fn changing_yuv_metadata_resets_processing_history() {
    use prysm_capture::{YuvRange, YuvStandardMatrix};
    let config = Config {
        brightness_percent: 100,
        remove_black_bars: false,
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    let mut frame = Frame::new(
        [235, 128, 235, 128].repeat(64 * 36 / 2),
        64,
        36,
        PixelFormat::YUYV,
    );
    let full = processor.process_frame(frame.clone());
    frame.yuv_range = YuvRange::Limited;
    frame.yuv_matrix = YuvStandardMatrix::Bt709;
    let limited = processor.process_frame(frame.clone());
    assert!(full.top.sample_at(0.5).r < 1.0);
    assert!(limited.top.sample_at(0.5).r > 0.99);
    assert_eq!(limited, PrysmProcessor::new(&config).process_frame(frame));
}

#[test]
fn long_smoothing_duration_converges_instead_of_freezing() {
    let config = Config {
        brightness_percent: 100,
        smoothing_seconds: 10.0,
        remove_black_bars: false,
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    let start = Instant::now();
    processor.process_frame_at(rgb(8, 8, 255), start);
    let black = rgb(8, 8, 0);
    let mut output = processor.process_frame_at(black.clone(), start + Duration::from_millis(10));
    assert!(output.top.sample_at(0.5).r < 1.0);
    for step in 2..=3000 {
        output =
            processor.process_frame_at(black.clone(), start + Duration::from_millis(step * 10));
    }
    assert!(output.top.sample_at(0.5).r < 0.001);
}

#[test]
fn small_color_changes_are_sampled_on_every_frame() {
    let config = Config {
        smoothing_seconds: 0.0,
        remove_black_bars: false,
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    let mut previous = processor.process_frame(rgb(64, 36, 128));
    for value in 129..=135 {
        let frame = rgb(64, 36, value);
        let actual = processor.process_frame(frame.clone());
        assert_ne!(actual, previous);
        assert_eq!(actual, PrysmProcessor::new(&config).process_frame(frame));
        previous = actual;
    }
}

#[test]
fn smoothing_seconds_controls_transition_speed() {
    for (smoothing_seconds, expected) in [
        (0.0, 0.0),
        (0.1, 0.05),
        (0.2, 0.05_f32.sqrt()),
        (1.0, 0.05_f32.powf(0.1)),
        (-1.0, 0.0),
        (f32::NAN, 0.0),
        (f32::INFINITY, 0.0),
        (f32::NEG_INFINITY, 0.0),
    ] {
        let config = Config {
            brightness_percent: 100,
            smoothing_seconds,
            remove_black_bars: false,
            ..Config::default()
        };
        let mut processor = PrysmProcessor::new(&config);
        let start = Instant::now();
        processor.process_frame_at(rgb(64, 36, 255), start);
        let actual = processor
            .process_frame_at(rgb(64, 36, 0), start + Duration::from_millis(100))
            .top
            .sample_at(0.5)
            .r;
        assert!(
            (actual - expected).abs() < 1e-6,
            "smoothing {smoothing_seconds}s: {actual}"
        );
    }
}

#[test]
fn smoothing_duration_is_independent_of_frame_rate_and_dropped_frames() {
    let config = Config {
        brightness_percent: 100,
        smoothing_seconds: 1.0,
        remove_black_bars: false,
        ..Config::default()
    };
    let start = Instant::now();
    let schedules = [24, 30, 60, 120].map(|fps| {
        (1..=fps)
            .map(|i| Duration::from_secs_f64(f64::from(i) / f64::from(fps)))
            .collect::<Vec<_>>()
    });
    let dropped = [50, 200, 700, 1000].map(Duration::from_millis);
    for schedule in schedules
        .iter()
        .map(Vec::as_slice)
        .chain([dropped.as_slice()])
    {
        let mut processor = PrysmProcessor::new(&config);
        processor.process_frame_at(rgb(8, 8, 255), start);
        let mut output = None;
        for &elapsed in schedule {
            output = Some(processor.process_frame_at(rgb(8, 8, 0), start + elapsed));
        }
        let actual = output.unwrap().top.sample_at(0.5).r;
        assert!(
            (actual - 0.05).abs() < 1e-6,
            "{} frames: {actual}",
            schedule.len()
        );
    }
}

#[test]
fn repeated_or_earlier_timestamps_do_not_advance_smoothing() {
    let config = Config {
        brightness_percent: 100,
        smoothing_seconds: 1.0,
        remove_black_bars: false,
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    let start = Instant::now();
    let white = processor.process_frame_at(rgb(8, 8, 255), start);
    for now in [start, start - Duration::from_millis(100)] {
        assert_eq!(processor.process_frame_at(rgb(8, 8, 0), now), white);
    }
    let output = processor.process_frame_at(rgb(8, 8, 0), start + Duration::from_secs(1));
    assert!((output.top.sample_at(0.5).r - 0.05).abs() < 1e-6);
}

#[test]
fn configured_density_controls_valid_and_unsupported_frame_edge_colors() {
    for sample_density in [
        SampleDensity(0),
        SampleDensity(30),
        SampleDensity(150),
        SampleDensity(2000),
    ] {
        let config = Config {
            sample_density,
            remove_black_bars: false,
            ..Config::default()
        };
        let mut processor = PrysmProcessor::new(&config);
        for frame in [
            rgb(640, 360, 255),
            Frame::new(vec![], 640, 360, PixelFormat::MJPEG),
            rgb(0, 360, 128),
        ] {
            let width = frame.width as usize;
            let height = frame.height as usize;
            let output = processor.process_frame(frame);
            assert_eq!(output.top.len(), sample_density.samples_for_length(width));
            assert_eq!(output.bottom.len(), output.top.len());
            assert_eq!(output.left.len(), sample_density.samples_for_length(height));
            assert_eq!(output.right.len(), output.left.len());
        }
    }
}

#[test]
fn edge_depth_percentage_controls_how_much_picture_is_sampled() {
    let mut data = vec![0; 100 * 100 * 3];
    data[..100 * 10 * 3].fill(255);
    let frame = Frame::new(data, 100, 100, PixelFormat::RGB24);
    // The outer fraction f contributes 1 - (1 - f)^3 with quadratic falloff.
    for (edge_depth_percent, expected) in
        [(0, 1.0), (10, 1.0), (20, 0.875), (50, 0.488), (255, 0.488)]
    {
        let config = Config {
            brightness_percent: 100,
            smoothing_seconds: 0.0,
            edge_depth_percent,
            remove_black_bars: false,
            ..Config::default()
        };
        let actual = PrysmProcessor::new(&config)
            .process_frame(frame.clone())
            .top
            .sample_at(0.5)
            .r;
        assert!(
            (actual - expected).abs() < 0.001,
            "edge depth {edge_depth_percent}%: {actual}"
        );
    }
}

#[test]
fn black_bar_removal_can_be_disabled() {
    let mut data = vec![255; 640 * 360 * 3];
    data[..640 * 48 * 3].fill(0);
    data[640 * 312 * 3..].fill(0);
    let frame = Frame::new(data, 640, 360, PixelFormat::RGB24);
    for remove_black_bars in [false, true] {
        let config = Config {
            brightness_percent: 100,
            smoothing_seconds: 0.0,
            edge_depth_percent: 10,
            remove_black_bars,
            ..Config::default()
        };
        let mut processor = PrysmProcessor::new(&config);
        for _ in 0..60 {
            processor.process_frame(frame.clone());
        }
        let actual = processor.process_frame(frame.clone()).top.sample_at(0.5).r;
        let expected = if remove_black_bars { 1.0 } else { 0.0 };
        assert!((actual - expected).abs() < 1e-6);
    }
}

#[test]
fn confirmed_crop_resets_smoothing_to_the_new_sample_grid() {
    let config = Config {
        brightness_percent: 100,
        ..Config::default()
    };
    let mut smoothed = PrysmProcessor::new(&config);
    let mut direct = PrysmProcessor::new(&Config {
        smoothing_seconds: 0.0,
        ..config
    });
    let mut data = vec![128; 640 * 360 * 3];
    for y in 0..360 {
        for x in 0..640 {
            let value = if !(48..312).contains(&y) {
                0
            } else if x < 24 {
                if ((y - 48) / 38) % 2 == 0 { 255 } else { 0 }
            } else {
                128
            };
            data[(y * 640 + x) * 3..(y * 640 + x + 1) * 3].fill(value);
        }
    }
    let letterbox = Frame::new(data, 640, 360, PixelFormat::RGB24);
    for _ in 0..80 {
        let actual = smoothed.process_frame(letterbox.clone());
        let expected = direct.process_frame(letterbox.clone());
        assert_eq!(actual.left.len(), expected.left.len());
        for i in 0u8..=100 {
            let position = f32::from(i) / 100.0;
            assert!(
                (actual.left.sample_at(position).r - expected.left.sample_at(position).r).abs()
                    < 0.001
            );
        }
    }
}
