use prysm_core::{
    BrightnessPercent, Config, Frame, PixelFormat, SampleDensity, SmoothingSeconds,
    SpatialReachPercent,
};
use prysm_processor::PrysmProcessor;
use std::time::{Duration, Instant};

fn rgb(width: u32, height: u32, value: u8) -> Frame {
    Frame::fill(value, width, height, PixelFormat::RGB24).unwrap()
}

#[test]
fn live_shadow_lift_preserves_history_and_respects_master_brightness() {
    let start = Instant::now();
    for brightness in [0, 50, 100] {
        let config = Config {
            remove_black_bars: false,
            brightness_percent: BrightnessPercent(brightness),
            ..Config::default()
        };
        let mut processor = PrysmProcessor::new(&config);
        let mut reference = PrysmProcessor::new(&Config {
            brightness_percent: BrightnessPercent(100),
            ..config
        });
        processor.process_frame_at(rgb(64, 36, 10), start);
        reference.process_frame_at(rgb(64, 36, 10), start);
        for (step, strength) in [100, 0, 50].into_iter().enumerate() {
            let strength = prysm_core::ShadowLiftPercent(strength);
            processor.configure(&Config {
                shadow_lift_percent: strength,
                ..config
            });
            let now = start + Duration::from_millis((step as u64 + 1) * 10);
            let actual = processor
                .process_frame_at(rgb(64, 36, 30), now)
                .top
                .sample_at(0.5);
            let expected = reference
                .process_frame_at(rgb(64, 36, 30), now)
                .top
                .sample_at(0.5)
                .lift_shadows(strength)
                * config.brightness_percent.as_ratio();
            assert!(
                (actual.r - expected.r).abs() < 1e-6,
                "history reset or brightness bypassed"
            );
        }
    }
}

#[test]
fn a_static_letterboxed_picture_does_not_bypass_smoothing_when_crop_confirms() {
    let config = Config::default();
    let mut data = vec![128; 640 * 360 * 3];
    data[..640 * 48 * 3].fill(0);
    data[640 * 312 * 3..].fill(0);
    let frame = Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap();
    let mut processor = PrysmProcessor::new(&config);
    let start = Instant::now();
    let before = processor.process_frame_at(frame.clone(), start);
    for millis in (10..200).step_by(10) {
        processor.process_frame_at(frame.clone(), start + Duration::from_millis(millis));
    }
    let after = processor.process_frame_at(frame, start + Duration::from_millis(200));
    let old = before.top.sample_at(0.5).r;
    let target = prysm_core::LinearColor::from_srgb(prysm_core::Color::new(128, 128, 128)).r;
    let actual = after.top.sample_at(0.5).r;
    let expected = old + (target - old) * (1.0 - 0.05_f32.powf(0.01 / 0.1));
    assert!(
        (actual - expected).abs() < 1e-5,
        "unchanged frame: top {old:.4} -> {actual:.4}; expected smoothed {expected:.4}, target {target:.4}"
    );
}

#[test]
fn edge_highlights_keep_local_contrast_and_interior_lights_still_contribute() {
    let config = Config {
        remove_black_bars: false,
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default()
    };
    for edge in 0..5 {
        let mut data = vec![0; 640 * 360 * 3];
        for y in 0..360 {
            for x in 0..640 {
                let lit = match edge {
                    0 => (308..332).contains(&x) && y < 24,
                    1 => x >= 616 && (168..192).contains(&y),
                    2 => (308..332).contains(&x) && y >= 336,
                    3 => x < 24 && (168..192).contains(&y),
                    _ => (308..332).contains(&x) && (168..192).contains(&y),
                };
                if lit {
                    data[(y * 640 + x) * 3] = 255;
                }
            }
        }
        let colors = PrysmProcessor::new(&config)
            .process_frame(Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap());
        let edges = [&colors.top, &colors.right, &colors.bottom, &colors.left];
        if edge < 4 {
            let nearby = edges[edge].sample_at(0.5).r;
            // A 24px highlight should retain useful contrast, not become a faint room wash.
            assert!(nearby > 0.25, "edge {edge}: highlight reduced to {nearby}");
            assert!(
                nearby < 0.8,
                "edge {edge}: highlight overamplified to {nearby}"
            );
            assert!(nearby > edges[edge].sample_at(0.0).r * 10.0);
        } else {
            // Keep a measurable scene contribution, not just a mathematically nonzero tail.
            assert!(colors.top.sample_at(0.5).r > 0.001);
            assert!(colors.bottom.sample_at(0.5).r > 0.001);
            assert!(colors.left.sample_at(0.5).r > 0.0003);
            assert!(colors.right.sample_at(0.5).r > 0.0003);
        }
    }
}

fn settle(processor: &mut PrysmProcessor, frame: &Frame, start: Instant) -> prysm_core::EdgeColors {
    (0..=60)
        .map(|i| processor.process_frame_at(frame.clone(), start + Duration::from_millis(i * 50)))
        .last()
        .unwrap()
}

#[test]
fn combined_and_thin_black_bars_follow_the_picture() {
    let config = Config {
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default()
    };
    for (x, y, width, height) in [
        (219, 48, 202, 264),
        (240, 130, 160, 100),
        (80, 130, 480, 100),
        (12, 7, 601, 340),
        (12, 0, 616, 360),
        (0, 7, 640, 346),
    ] {
        let mut data = vec![0; 640 * 360 * 3];
        for row in y..y + height {
            data[(row * 640 + x) * 3..(row * 640 + x + width) * 3].fill(255);
        }
        let frame = Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap();
        let output = settle(&mut PrysmProcessor::new(&config), &frame, Instant::now());
        assert_eq!(
            output.top.len(),
            config.sample_density.samples_for_length(width)
        );
        assert_eq!(
            output.left.len(),
            config.sample_density.samples_for_length(height)
        );
        for edge in [&output.top, &output.right, &output.bottom, &output.left] {
            assert!(
                edge.sample_at(0.5).r > 0.99,
                "picture {x},{y} {width}x{height}"
            );
        }
    }
}

#[test]
fn combined_bars_keep_dark_centers_and_bright_corners_inside_the_picture() {
    let config = Config {
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default()
    };
    for pattern in 0..5 {
        let mut data = vec![0; 640 * 360 * 3];
        let mut picture = vec![0; 160 * 100 * 3];
        for y in 0..100 {
            for x in 0..160 {
                let bright = match pattern {
                    0 => !(72..88).contains(&x),
                    1 => !(42..58).contains(&y),
                    2 => !(72..88).contains(&x) && !(42..58).contains(&y),
                    _ => {
                        let corner_width = if pattern == 4 { 30 } else { 40 };
                        (!(corner_width..160 - corner_width).contains(&x) && !(30..70).contains(&y))
                            || ((40..120).contains(&x) && (30..70).contains(&y))
                    }
                };
                let value = if bright { 128 } else { 0 };
                let frame_index = ((y + 130) * 640 + x + 240) * 3;
                data[frame_index..frame_index + 3].fill(value);
                let picture_index = (y * 160 + x) * 3;
                picture[picture_index..picture_index + 3].fill(value);
            }
        }
        let frame = Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap();
        let actual = settle(&mut PrysmProcessor::new(&config), &frame, Instant::now());
        let expected_frame = Frame::new(picture, 160, 100, PixelFormat::RGB24).unwrap();
        let expected = PrysmProcessor::new(&Config {
            remove_black_bars: false,
            ..config
        })
        .process_frame(expected_frame);
        assert_eq!(actual.top.len(), expected.top.len(), "pattern {pattern}");
        assert_eq!(actual.left.len(), expected.left.len(), "pattern {pattern}");
        assert!(actual == expected, "pattern {pattern} lost picture content");
    }
}

#[test]
fn dim_gray_and_colored_edges_are_picture_not_black_bars() {
    let config = Config {
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default()
    };
    for format in [PixelFormat::RGB24, PixelFormat::BGR24] {
        for edge_color in [[40, 40, 40], [0, 0, 24]] {
            let mut data = edge_color.repeat(640 * 360);
            for row in 0..360 {
                data[(row * 640 + 80) * 3..(row * 640 + 560) * 3].fill(128);
            }
            let frame = Frame::new(data, 640, 360, format).unwrap();
            let output = settle(&mut PrysmProcessor::new(&config), &frame, Instant::now());
            let expected = PrysmProcessor::new(&Config {
                remove_black_bars: false,
                ..config
            })
            .process_frame(frame);
            assert!(output == expected, "{format:?}, edges {edge_color:?}");
        }
    }
}

#[test]
fn stream_preserves_typed_source_errors() {
    use futures::StreamExt;
    futures::executor::block_on(async {
        let frames = futures::stream::iter([
            Ok(rgb(2, 2, 255)),
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        ]);
        let colors = PrysmProcessor::default().into_stream(frames);
        futures::pin_mut!(colors);
        assert!(colors.next().await.unwrap().is_ok());
        assert_eq!(
            colors.next().await.unwrap().unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(colors.next().await.is_none());
    });
}

#[test]
fn wide_black_bands_crop_each_axis_independently() {
    let config = Config {
        brightness_percent: BrightnessPercent(100),
        smoothing_seconds: SmoothingSeconds(0.0),
        sample_density: SampleDensity(150),
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    let mut start = Instant::now();
    for (x_start, x_end, y_start, y_end) in [(219, 421, 0, 360), (0, 640, 130, 230)] {
        let mut data = vec![0; 640 * 360 * 3];
        for y in y_start..y_end {
            data[(y * 640 + x_start) * 3..(y * 640 + x_end) * 3].fill(255);
        }
        let frame = Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap();
        let output = settle(&mut processor, &frame, start);
        start += Duration::from_secs(4);
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
    use prysm_core::YuvRange;
    let config = Config {
        brightness_percent: BrightnessPercent(100),
        smoothing_seconds: SmoothingSeconds(0.0),
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
            let frame = Frame::new(
                data,
                640,
                360,
                PixelFormat::YUYV {
                    range,
                    matrix: prysm_core::YuvStandardMatrix::Bt601,
                },
            )
            .unwrap();
            let mut processor = PrysmProcessor::new(&config);
            let output = settle(&mut processor, &frame, Instant::now());
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
        sample_density: SampleDensity(60),
        brightness_percent: BrightnessPercent(100),
        smoothing_seconds: SmoothingSeconds(0.0),
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
        let output = PrysmProcessor::new(&config).process_frame(
            Frame::new(data, width as u32, height as u32, PixelFormat::RGB24).unwrap(),
        );
        let edges = if width == 636 {
            [&output.top, &output.bottom]
        } else {
            [&output.left, &output.right]
        };
        for edge in edges {
            assert!(edge.sample_at(0.0).r > 0.0);
            assert!(edge.sample_at(1.0).r > edge.sample_at(0.0).r);
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
        brightness_percent: BrightnessPercent(100),
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
        brightness_percent: BrightnessPercent(100),
        remove_black_bars: false,
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    let a = Frame::new(
        [128, 128, 128, 128].repeat(64 * 36 / 2),
        64,
        36,
        PixelFormat::YUYV {
            range: prysm_core::YuvRange::Full,
            matrix: prysm_core::YuvStandardMatrix::Bt601,
        },
    )
    .unwrap();
    let b = Frame::new(
        [128, 128, 128, 200].repeat(64 * 36 / 2),
        64,
        36,
        PixelFormat::YUYV {
            range: prysm_core::YuvRange::Full,
            matrix: prysm_core::YuvStandardMatrix::Bt601,
        },
    )
    .unwrap();
    p.process_frame(a.clone());
    p.process_frame(a);
    let actual = p.process_frame(b.clone());
    let expected = PrysmProcessor::new(&c).process_frame(b);
    assert_eq!(actual, expected, "same luma, changed chroma was skipped");
}

#[test]
fn static_letterbox_confirms_with_default_tuning() {
    let c = Config {
        brightness_percent: BrightnessPercent(100),
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default()
    };
    let mut data = vec![128; 640 * 360 * 3];
    data[..640 * 48 * 3].fill(0);
    data[640 * 312 * 3..].fill(0);
    let frame = Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap();
    let mut p = PrysmProcessor::new(&c);
    assert_eq!(p.viewport(), None);
    let actual = settle(&mut p, &frame, Instant::now());
    assert_eq!(
        p.viewport(),
        Some(prysm_core::Viewport {
            x: 0,
            y: 48,
            width: 640,
            height: 264,
        })
    );
    assert!(
        actual.top.sample_at(0.5).r > 0.1,
        "letterbox still unconfirmed after three seconds"
    );
}

#[test]
fn resize_must_update_sample_count() {
    let c = Config {
        brightness_percent: BrightnessPercent(100),
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
        brightness_percent: BrightnessPercent(100),
        remove_black_bars: false,
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    let actual = p.process_frame(
        Frame::new([0, 0, 255].repeat(64 * 36), 64, 36, PixelFormat::BGR24).unwrap(),
    );
    let color = actual.top.sample_at(0.5);
    assert!(
        color.r > 0.99 && color.g == 0.0 && color.b == 0.0,
        "BGR red was decoded incorrectly: {color:?}"
    );
}

#[test]
fn dark_scene_preserves_confirmed_crop() {
    let c = Config {
        brightness_percent: BrightnessPercent(100),
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    let mut data = vec![128; 640 * 360 * 3];
    data[..640 * 48 * 3].fill(0);
    data[640 * 312 * 3..].fill(0);
    let letterbox = Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap();
    let start = Instant::now();
    let cropped = settle(&mut p, &letterbox, start);
    assert_eq!(
        cropped.left.len(),
        SampleDensity::default().samples_for_length(264)
    );
    for i in 0..60 {
        let dark = p.process_frame_at(rgb(640, 360, 20), start + Duration::from_secs(4 + i));
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
    let dark = p.process_frame(Frame::fill(0, 64, 36, PixelFormat::BGR24).unwrap());
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
        brightness_percent: BrightnessPercent(100),
        remove_black_bars: false,
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default()
    };
    let mut p = PrysmProcessor::new(&c);
    let red = Frame::new([255, 0, 0].repeat(64 * 36), 64, 36, PixelFormat::RGB24).unwrap();
    let blue = Frame::new([0, 0, 255].repeat(64 * 36), 64, 36, PixelFormat::RGB24).unwrap();
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
            brightness_percent: BrightnessPercent(brightness_percent),
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
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    let base = rgb(640, 360, 128);
    let mut data = base.as_slice().to_vec();
    data[..640 * 10 * 3].fill(255);
    let changed = Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap();
    let old = processor.process_frame(base);
    let actual = processor.process_frame(changed.clone());
    let expected = PrysmProcessor::new(&config).process_frame(changed);
    assert_ne!(actual, old);
    assert_eq!(actual, expected);
}

#[test]
fn bright_objects_grow_stronger_as_they_approach_each_edge() {
    let config = Config {
        brightness_percent: BrightnessPercent(100),
        smoothing_seconds: SmoothingSeconds(0.0),
        remove_black_bars: false,
        spatial_reach_percent: SpatialReachPercent(15),
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
                let output = processor
                    .process_frame(Frame::new(data, width, height, PixelFormat::RGB24).unwrap());
                let edges = [&output.top, &output.right, &output.bottom, &output.left];
                let actual = edges[edge].sample_at(0.5);
                assert!(
                    actual.r > previous,
                    "{width}x{height}, edge {edge}, inset {inset_percent}%: {actual:?} <= {previous}"
                );
                assert!(actual.r < 0.5, "small highlights must stay dim: {actual:?}");
                assert_eq!((actual.g, actual.b), (0.0, 0.0));
                let opposite = edges[(edge + 2) % 4].sample_at(0.5).r;
                assert!(opposite > 0.0 && opposite < actual.r);
                previous = actual.r;
            }
        }
    }
}

#[test]
fn changing_yuv_metadata_resets_processing_history() {
    use prysm_core::{YuvRange, YuvStandardMatrix};
    let config = Config {
        brightness_percent: BrightnessPercent(100),
        remove_black_bars: false,
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    let mut frame = Frame::new(
        [235, 128, 235, 128].repeat(64 * 36 / 2),
        64,
        36,
        PixelFormat::YUYV {
            range: prysm_core::YuvRange::Full,
            matrix: prysm_core::YuvStandardMatrix::Bt601,
        },
    )
    .unwrap();
    let full = processor.process_frame(frame.clone());
    frame = Frame::new(
        frame.as_slice().to_vec(),
        frame.width(),
        frame.height(),
        PixelFormat::YUYV {
            range: YuvRange::Limited,
            matrix: YuvStandardMatrix::Bt709,
        },
    )
    .unwrap();
    let limited = processor.process_frame(frame.clone());
    assert!(full.top.sample_at(0.5).r < 1.0);
    assert!(limited.top.sample_at(0.5).r > 0.99);
    assert_eq!(limited, PrysmProcessor::new(&config).process_frame(frame));
}

#[test]
fn long_smoothing_duration_converges_instead_of_freezing() {
    let config = Config {
        brightness_percent: BrightnessPercent(100),
        smoothing_seconds: SmoothingSeconds(10.0),
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
        smoothing_seconds: SmoothingSeconds(0.0),
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
            brightness_percent: BrightnessPercent(100),
            smoothing_seconds: SmoothingSeconds(smoothing_seconds),
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
        brightness_percent: BrightnessPercent(100),
        smoothing_seconds: SmoothingSeconds(1.0),
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
        brightness_percent: BrightnessPercent(100),
        smoothing_seconds: SmoothingSeconds(1.0),
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
            Frame::new(vec![], 640, 360, PixelFormat::MJPEG).unwrap(),
            rgb(0, 360, 128),
        ] {
            let width = frame.width() as usize;
            let height = frame.height() as usize;
            let output = processor.process_frame(frame);
            assert_eq!(output.top.len(), sample_density.samples_for_length(width));
            assert_eq!(output.bottom.len(), output.top.len());
            assert_eq!(output.left.len(), sample_density.samples_for_length(height));
            assert_eq!(output.right.len(), output.left.len());
        }
    }
}

#[test]
fn spatial_reach_controls_interior_influence_and_clamps_extremes() {
    let mut data = vec![0; 100 * 100 * 3];
    for y in 40..60 {
        for x in 40..60 {
            data[(y * 100 + x) * 3] = 255;
        }
    }
    let frame = Frame::new(data, 100, 100, PixelFormat::RGB24).unwrap();
    let sample = |reach| {
        PrysmProcessor::new(&Config {
            smoothing_seconds: SmoothingSeconds(0.0),
            spatial_reach_percent: SpatialReachPercent(reach),
            remove_black_bars: false,
            ..Config::default()
        })
        .process_frame(frame.clone())
        .top
        .sample_at(0.5)
        .r
    };
    let values = [1, 10, 30, 100].map(sample);
    assert!(values[0] > 0.0);
    assert!(
        values.windows(2).all(|pair| pair[1] > pair[0]),
        "{values:?}"
    );
    assert_eq!(sample(0), sample(1));
    assert_eq!(sample(255), sample(100));
}

#[test]
fn black_bar_removal_can_be_disabled() {
    let mut data = vec![255; 640 * 360 * 3];
    data[..640 * 48 * 3].fill(0);
    data[640 * 312 * 3..].fill(0);
    let frame = Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap();
    for remove_black_bars in [false, true] {
        let config = Config {
            brightness_percent: BrightnessPercent(100),
            smoothing_seconds: SmoothingSeconds(0.0),
            spatial_reach_percent: SpatialReachPercent(10),
            remove_black_bars,
            ..Config::default()
        };
        let mut processor = PrysmProcessor::new(&config);
        let actual = settle(&mut processor, &frame, Instant::now())
            .top
            .sample_at(0.5)
            .r;
        if remove_black_bars {
            assert!((actual - 1.0).abs() < 1e-6);
        } else {
            assert!(actual > 0.0 && actual < 0.9, "uncropped bars: {actual}");
        }
    }
}

#[test]
fn crop_changes_preserve_smoothing_and_adopt_the_new_sample_grid() {
    let config = Config {
        brightness_percent: BrightnessPercent(100),
        ..Config::default()
    };
    let mut smoothed = PrysmProcessor::new(&config);
    let mut direct = PrysmProcessor::new(&Config {
        smoothing_seconds: SmoothingSeconds(0.0),
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
    let letterbox = Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap();
    let full = rgb(640, 360, 128);
    let start = Instant::now();
    let mut previous: Option<prysm_core::EdgeColors> = None;
    let ratio = 1.0 - 0.05_f32.powf(0.05 / config.smoothing_seconds.0);
    for step in 0..160 {
        let frame = if step < 80 { &letterbox } else { &full };
        let now = start + Duration::from_millis(step * 50);
        let actual = smoothed.process_frame_at(frame.clone(), now);
        let target = direct.process_frame_at(frame.clone(), now);
        for (actual, target, previous) in [
            (&actual.top, &target.top, previous.as_ref().map(|p| &p.top)),
            (
                &actual.left,
                &target.left,
                previous.as_ref().map(|p| &p.left),
            ),
        ] {
            assert_eq!(actual.len(), target.len());
            for index in 0..actual.len() {
                let goal = target.color_at(index, actual.len()).r;
                let expected = previous.map_or(goal, |old| {
                    let old = old.color_at(index, actual.len()).r;
                    old + (goal - old) * ratio
                });
                assert!(
                    (actual.color_at(index, actual.len()).r - expected).abs() < 1e-5,
                    "frame {step}, sample {index}: crop transition bypassed smoothing"
                );
            }
        }
        if step == 79 {
            assert!(actual.left.len() < config.sample_density.samples_for_length(360));
        } else if step == 159 {
            assert_eq!(
                actual.left.len(),
                config.sample_density.samples_for_length(360)
            );
        }
        previous = Some(actual);
    }
}

#[test]
fn configuring_brightness_and_smoothing_duration_keeps_transition_history() {
    let config = Config {
        remove_black_bars: false,
        smoothing_seconds: SmoothingSeconds(1.0),
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    let start = Instant::now();
    processor.process_frame_at(rgb(100, 60, 0), start);
    processor.configure(&Config {
        brightness_percent: BrightnessPercent(50),
        smoothing_seconds: SmoothingSeconds(0.5),
        ..config
    });
    let colors = processor.process_frame_at(rgb(100, 60, 255), start + Duration::from_millis(100));
    let expected = (1.0 - 0.05_f32.powf(0.1 / 0.5)) * 0.5;
    assert!((colors.top.sample_at(0.5).r - expected).abs() < 1e-6);
}

#[test]
fn configuring_sampling_rebuilds_geometry_and_preserves_smoothing_history() {
    let config = Config {
        remove_black_bars: false,
        sample_density: SampleDensity(10),
        spatial_reach_percent: SpatialReachPercent(1),
        smoothing_seconds: SmoothingSeconds(0.5),
        ..Config::default()
    };
    let mut processor = PrysmProcessor::new(&config);
    let start = Instant::now();
    processor.process_frame_at(rgb(160, 90, 0), start);
    let updated = Config {
        sample_density: SampleDensity(200),
        spatial_reach_percent: SpatialReachPercent(40),
        ..config
    };
    processor.configure(&updated);
    let mut data = vec![0; 160 * 90 * 3];
    for y in 30..60 {
        for x in 60..100 {
            data[(y * 160 + x) * 3] = 255;
        }
    }
    let frame = Frame::new(data, 160, 90, PixelFormat::RGB24).unwrap();
    let actual = processor.process_frame_at(frame.clone(), start + Duration::from_millis(100));
    let target = PrysmProcessor::new(&updated).process_frame_at(frame, start);
    assert_eq!(actual.top.len(), 32);
    assert_eq!(actual.left.len(), 18);
    let ratio = 1.0 - 0.05_f32.powf(0.1 / 0.5);
    for (actual, target) in [(&actual.top, &target.top), (&actual.left, &target.left)] {
        for sample in 0..actual.len() {
            let expected = target.color_at(sample, actual.len()).r * ratio;
            assert!((actual.color_at(sample, actual.len()).r - expected).abs() < 1e-6);
        }
    }
}

#[test]
fn configuring_bar_removal_only_resets_detection_when_toggled() {
    let config = Config {
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default()
    };
    let mut data = vec![255; 160 * 90 * 3];
    data[..160 * 12 * 3].fill(0);
    data[160 * 78 * 3..].fill(0);
    let frame = Frame::new(data, 160, 90, PixelFormat::RGB24).unwrap();
    let mut processor = PrysmProcessor::new(&config);
    let start = Instant::now();
    settle(&mut processor, &frame, start);
    let cropped = processor.viewport().unwrap();
    assert_eq!(cropped.height, 66);
    processor.configure(&Config {
        brightness_percent: BrightnessPercent(50),
        ..config
    });
    processor.process_frame_at(frame.clone(), start + Duration::from_millis(3050));
    assert_eq!(
        processor.viewport(),
        Some(cropped),
        "brightness must not reset crop confirmation"
    );
    processor.configure(&Config {
        remove_black_bars: false,
        ..config
    });
    processor.process_frame_at(frame.clone(), start + Duration::from_millis(3100));
    assert_eq!(
        processor.viewport(),
        Some(prysm_core::Viewport::full_frame(160, 90))
    );
    processor.configure(&config);
    settle(&mut processor, &frame, start + Duration::from_millis(3150));
    assert_eq!(processor.viewport(), Some(cropped));
}
