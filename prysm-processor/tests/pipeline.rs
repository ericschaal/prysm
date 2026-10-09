use prysm_capture::{Frame, PixelFormat};
use prysm_core::Config;
use prysm_processor::PrysmProcessor;

fn rgb(width: u32, height: u32, value: u8) -> Frame {
    Frame::fill(value, width, height, PixelFormat::RGB24)
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
