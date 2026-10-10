use prysm_capture::{Frame, PixelFormat};
use prysm_core::Config;
use prysm_processor::PrysmProcessor;

fn rgb(width: u32, height: u32, value: u8) -> Frame {
    Frame::fill(value, width, height, PixelFormat::RGB24)
}

#[test]
fn wide_black_bands_crop_each_axis_independently() {
    let config = Config {
        brightness: 1.0,
        temporal_smoothing: 0.0,
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
        brightness: 1.0,
        temporal_smoothing: 0.0,
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
                config.sample_density.samples_for_length(width)
            );
            assert_eq!(
                output.left.len(),
                config.sample_density.samples_for_length(height)
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
        brightness: 1.0,
        temporal_smoothing: 0.0,
        black_band_detection: false,
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
            assert!((edge.sample_at(1.0).r - 1.0 / 34.0).abs() < 0.00001);
        }
    }
}

#[test]
fn one_scan_confirmation_applies_the_first_candidate() {
    let config = Config {
        band_confirm_frames: 1,
        band_detection_interval: 1,
        ..Config::default()
    };
    let mut data = vec![255; 640 * 360 * 3];
    data[..640 * 48 * 3].fill(0);
    data[640 * 312 * 3..].fill(0);
    let output =
        PrysmProcessor::new(&config).process_frame(Frame::new(data, 640, 360, PixelFormat::RGB24));
    assert_eq!(
        output.left.len(),
        config.sample_density.samples_for_length(264)
    );
}

#[test]
fn dark_frames_must_not_panic() {
    let mut p = PrysmProcessor::default();
    for _ in 0..1900 {
        let spectra = p.process_frame(rgb(64, 36, 20));
        assert!(spectra.top.sample_at(0.5).r > 0.0);
    }
}

#[test]
fn smoothing_must_keep_converging_on_static_input() {
    let c = Config {
        change_detection: true,
        brightness: 1.0,
        black_band_detection: false,
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    p.process_frame(rgb(64, 36, 255));
    let black = rgb(64, 36, 0);
    p.process_frame(black.clone());
    let first = p.process_frame(black.clone()).top.sample_at(0.5).r;
    let next = p.process_frame(black).top.sample_at(0.5).r;
    assert!(next < first, "smoothing stopped: {first} -> {next}");
}

#[test]
fn color_change_must_update_output() {
    let c = Config {
        change_detection: true,
        brightness: 1.0,
        black_band_detection: false,
        temporal_smoothing: 0.0,
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
fn static_letterbox_confirms_after_configured_scans() {
    let c = Config {
        change_detection: true,
        brightness: 1.0,
        temporal_smoothing: 0.0,
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
        brightness: 1.0,
        black_band_detection: false,
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
        brightness: 1.0,
        black_band_detection: false,
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
        brightness: 1.0,
        temporal_smoothing: 0.0,
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
    assert_eq!(cropped.left.len(), c.sample_density.samples_for_length(264));
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
        let spectra = p.process_frame(rgb(width, height, 128));
        assert!(spectra.top.sample_at(0.5).r.abs() < f32::EPSILON);
    }
}

#[test]
fn equal_mean_rgb_colors_are_detected() {
    let c = Config {
        change_detection: true,
        brightness: 1.0,
        black_band_detection: false,
        temporal_smoothing: 0.0,
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
    for brightness in [0.0, 0.5, 1.0] {
        let config = Config {
            brightness,
            black_band_detection: false,
            ..Config::default()
        };
        let mut processor = PrysmProcessor::new(&config);
        for _ in 0..3 {
            let color = processor.process_frame(rgb(64, 36, 255)).top.sample_at(0.5);
            assert!(
                (color.r - brightness).abs() < 1e-6,
                "brightness {brightness}: {color:?}"
            );
        }
    }
}

#[test]
fn default_processing_detects_narrow_edge_changes_immediately() {
    let config = Config {
        temporal_smoothing: 0.0,
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
fn zero_band_scan_settings_are_clamped() {
    let config = Config {
        band_detection_interval: 0,
        band_sample_stride: 0,
        ..Config::default()
    };
    let actual = PrysmProcessor::new(&config).process_frame(rgb(64, 36, 128));
    assert!(actual.top.sample_at(0.5).r > 0.0);
}

#[test]
fn changing_yuv_metadata_resets_processing_history() {
    use prysm_capture::{YuvRange, YuvStandardMatrix};
    let config = Config {
        brightness: 1.0,
        change_detection: true,
        black_band_detection: false,
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
fn maximum_smoothing_converges_instead_of_freezing() {
    let config = Config {
        brightness: 1.0,
        temporal_smoothing: 1.0,
        black_band_detection: false,
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    processor.process_frame(rgb(8, 8, 255));
    let black = rgb(8, 8, 0);
    let mut output = processor.process_frame(black.clone());
    assert!(output.top.sample_at(0.5).r < 1.0);
    for _ in 0..1000 {
        output = processor.process_frame(black.clone());
    }
    assert!(output.top.sample_at(0.5).r < 0.001);
}

#[test]
fn oversampling_a_white_frame_does_not_create_black_samples() {
    let config = Config {
        sample_density: prysm_core::SampleDensity(2000),
        brightness: 1.0,
        temporal_smoothing: 0.0,
        black_band_detection: false,
        ..Config::default()
    };
    let output = PrysmProcessor::new(&config).process_frame(rgb(8, 8, 255));
    for edge in [&output.top, &output.bottom, &output.left, &output.right] {
        for color in edge.quantize(edge.len()) {
            assert_eq!(color, prysm_core::LinearColor::new(1.0, 1.0, 1.0));
        }
        assert_eq!(edge.len(), 8);
    }
}

#[test]
fn confirmed_crop_resets_smoothing_to_the_new_sample_grid() {
    let config = Config {
        brightness: 1.0,
        ..Config::default()
    };
    let mut smoothed = PrysmProcessor::new(&config);
    let mut direct = PrysmProcessor::new(&Config {
        temporal_smoothing: 0.0,
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
