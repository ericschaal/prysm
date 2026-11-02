use super::BlackBarDetector;
use crate::frames::{FrameView, Viewport, yuyv_frame_from_luma};
use prysm_core::{Frame, PixelFormat, YuvRange, YuvStandardMatrix};
use std::time::{Duration, Instant};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;

fn picture(bars: [u32; 4], pixel: impl Fn(u32, u32) -> u8) -> Frame {
    let [top, right, bottom, left] = bars;
    yuyv_frame_from_luma(WIDTH, HEIGHT, |x, y| {
        if x < left || x >= WIDTH - right || y < top || y >= HEIGHT - bottom {
            0
        } else {
            pixel(x, y)
        }
    })
}

fn with_range(frame: &Frame, range: YuvRange) -> Frame {
    if range == YuvRange::Full {
        return frame.clone();
    }
    let mut bytes = frame.as_slice().to_vec();
    for y in bytes.iter_mut().step_by(2) {
        *y = 16 + u8::try_from(u16::from(*y) * 219 / 255).unwrap();
    }
    Frame::new(
        bytes,
        WIDTH,
        HEIGHT,
        PixelFormat::YUYV {
            range,
            matrix: YuvStandardMatrix::Bt709,
        },
    )
    .unwrap()
}

fn viewport(bars: [u32; 4]) -> Viewport {
    let [top, right, bottom, left] = bars;
    Viewport {
        x: left,
        y: top,
        width: WIDTH - left - right,
        height: HEIGHT - top - bottom,
    }
}

fn at(detector: &mut BlackBarDetector, frame: &Frame, start: Instant, millis: u64) -> Viewport {
    detector
        .process(
            FrameView::new(frame.clone()),
            start + Duration::from_millis(millis),
        )
        .viewport
}

fn feed(
    detector: &mut BlackBarDetector,
    frame: &Frame,
    start: Instant,
    first: u64,
    last: u64,
) -> Viewport {
    let mut output = viewport([0; 4]);
    for millis in (first..=last).step_by(50) {
        output = at(detector, frame, start, millis);
    }
    output
}

#[test]
fn one_pixel_bottom_noise_never_moves_the_top_edge() {
    let start = Instant::now();
    let mut detector = BlackBarDetector::new();
    let boxed = picture([20, 0, 11, 0], |_, _| 128);
    assert_eq!(
        feed(&mut detector, &boxed, start, 0, 2000),
        viewport([20, 0, 11, 0])
    );
    for (phase, bottom) in [10, 11, 10, 11].into_iter().enumerate() {
        let frame = picture([20, 0, bottom, 0], |_, _| 128);
        for millis in ((2050 + phase as u64 * 2000)..=(4000 + phase as u64 * 2000)).step_by(50) {
            let crop = at(&mut detector, &frame, start, millis);
            assert_eq!(
                crop.y, 20,
                "one pixel of bottom noise moved the top: {crop:?}"
            );
            assert!((10..=11).contains(&(HEIGHT - crop.y - crop.height)));
        }
    }
}

#[test]
fn asymmetric_thin_letter_pillar_and_window_bars_are_detected() {
    for bars in [
        [20, 0, 10, 0],
        [2, 0, 6, 0],
        [0, 90, 0, 64],
        [32, 80, 48, 60],
        [20, 0, 0, 0],
        [0; 4],
    ] {
        let full = picture(bars, |_, _| 128);
        for range in [YuvRange::Full, YuvRange::Limited] {
            let frame = with_range(&full, range);
            let crop = feed(
                &mut BlackBarDetector::new(),
                &frame,
                Instant::now(),
                0,
                2000,
            );
            assert_eq!(crop, viewport(bars), "{bars:?}, {range:?}");
        }
    }
}

#[test]
fn dim_picture_edges_do_not_consume_a_confirmed_picture() {
    let start = Instant::now();
    let boxed = picture([20, 0, 20, 0], |_, _| 128);
    let expected = viewport([20, 0, 20, 0]);
    let dim = picture(
        [20, 0, 20, 0],
        |_, y| {
            if (40..320).contains(&y) { 128 } else { 6 }
        },
    );
    for range in [YuvRange::Full, YuvRange::Limited] {
        let mut detector = BlackBarDetector::new();
        let boxed = with_range(&boxed, range);
        let dim = with_range(&dim, range);
        assert_eq!(feed(&mut detector, &boxed, start, 0, 2000), expected);
        for millis in (2050..=12_050).step_by(50) {
            assert_eq!(
                at(&mut detector, &dim, start, millis),
                expected,
                "dim detail became a bar at {millis}ms, {range:?}"
            );
        }
    }
}

#[test]
fn sustained_wide_subtitles_and_corner_logos_do_not_change_framing() {
    let start = Instant::now();
    let expected = viewport([48, 80, 48, 80]);
    let clean = picture([48, 80, 48, 80], |_, _| 128);
    let overlay = yuyv_frame_from_luma(WIDTH, HEIGHT, |x, y| {
        if ((80..560).contains(&x) && (48..312).contains(&y))
            || ((48..592).contains(&x) && (326..338).contains(&y) && x % 12 < 8)
            || ((12..56).contains(&x) && (4..18).contains(&y))
        {
            128
        } else {
            0
        }
    });
    for initial in [&clean, &overlay] {
        let mut detector = BlackBarDetector::new();
        assert_eq!(feed(&mut detector, initial, start, 0, 2000), expected);
        for millis in (2050..=6050).step_by(50) {
            let frame = if millis < 5050 { &overlay } else { &clean };
            assert_eq!(
                at(&mut detector, frame, start, millis),
                expected,
                "overlay moved crop at {millis}ms"
            );
        }
    }
}

#[test]
fn darkness_and_fades_keep_the_last_crop_without_confirming_new_bars() {
    let start = Instant::now();
    let boxed = picture([48, 0, 48, 0], |_, _| 128);
    let dark = yuyv_frame_from_luma(WIDTH, HEIGHT, |_, _| 0);
    let faded = picture([48, 0, 48, 0], |_, _| 4);
    let mut detector = BlackBarDetector::new();
    assert_eq!(feed(&mut detector, &dark, start, 0, 2000), viewport([0; 4]));
    assert_eq!(
        feed(&mut detector, &boxed, start, 2050, 4050),
        viewport([48, 0, 48, 0])
    );
    for (frame, begin) in [(&faded, 4100), (&dark, 7150)] {
        assert_eq!(
            feed(&mut detector, frame, start, begin, begin + 3000),
            viewport([48, 0, 48, 0])
        );
    }
    let mut candidate = BlackBarDetector::new();
    feed(&mut candidate, &boxed, start, 0, 100);
    feed(&mut candidate, &dark, start, 150, 3000);
    assert_eq!(at(&mut candidate, &boxed, start, 3050), viewport([0; 4]));
}

#[test]
fn genuine_aspect_changes_are_slow_inward_and_fast_outward_at_different_rates() {
    let start = Instant::now();
    let shallow = picture([20, 0, 20, 0], |_, _| 128);
    let boxed = picture([48, 0, 48, 0], |_, _| 128);
    let full = picture([0; 4], |_, _| 128);
    for fps in [24, 30, 60, 120] {
        let mut detector = BlackBarDetector::new();
        for (frame, offset, expected, earliest, latest) in [
            (&shallow, 0.0, viewport([20, 0, 20, 0]), 1.5, 2.0),
            (&boxed, 2.1, viewport([48, 0, 48, 0]), 1.5, 2.0),
            (&full, 4.2, viewport([0; 4]), 0.3, 0.55),
        ] {
            let mut first_change = None;
            for i in 0..=2 * fps {
                let elapsed = f64::from(i) / f64::from(fps);
                let crop = detector
                    .process(
                        FrameView::new(frame.clone()),
                        start + Duration::from_secs_f64(offset + elapsed),
                    )
                    .viewport;
                if crop == expected && first_change.is_none() {
                    first_change = Some(elapsed);
                }
            }
            let elapsed = first_change.expect("crop did not follow a genuine aspect change");
            assert!(
                (earliest..=latest).contains(&elapsed),
                "{fps}fps, offset {offset}: changed after {elapsed}s"
            );
        }
    }
}

#[test]
fn interrupted_evidence_does_not_turn_elapsed_time_into_confirmation() {
    let start = Instant::now();
    let boxed = picture([48, 0, 48, 0], |_, _| 128);
    let mut detector = BlackBarDetector::new();
    feed(&mut detector, &boxed, start, 0, 100);
    assert_eq!(at(&mut detector, &boxed, start, 10_000), viewport([0; 4]));
    assert_eq!(
        feed(&mut detector, &boxed, start, 10_050, 11_000),
        viewport([0; 4])
    );
    assert_eq!(
        feed(&mut detector, &boxed, start, 11_050, 12_000),
        viewport([48, 0, 48, 0])
    );
}

#[test]
fn repeated_or_earlier_times_do_not_advance_or_reset_evidence() {
    let start = Instant::now();
    let boxed = picture([48, 0, 48, 0], |_, _| 128);
    let full = picture([0; 4], |_, _| 128);
    let mut detector = BlackBarDetector::new();
    let before = feed(&mut detector, &boxed, start, 0, 1000);
    for now in [
        start.checked_sub(Duration::from_secs(1)).unwrap(),
        start,
        start + Duration::from_millis(1000),
    ] {
        assert_eq!(
            detector.process(FrameView::new(full.clone()), now).viewport,
            before
        );
    }
    assert_eq!(
        feed(&mut detector, &boxed, start, 1050, 1600),
        viewport([48, 0, 48, 0])
    );
}

#[test]
fn isolated_highlights_cannot_establish_a_picture_rectangle() {
    for cross in [false, true] {
        let frame = yuyv_frame_from_luma(WIDTH, HEIGHT, |x, y| {
            if (270..370).contains(&x)
                && (130..230).contains(&y)
                && (if cross {
                    x == 320 || y == 180
                } else {
                    x.abs_diff(320) < 5 && y.abs_diff(180) < 5
                })
            {
                128
            } else {
                0
            }
        });
        assert_eq!(
            feed(
                &mut BlackBarDetector::new(),
                &frame,
                Instant::now(),
                0,
                2000
            ),
            viewport([0; 4])
        );
    }
}

#[test]
fn alternating_framing_does_not_accumulate_false_support() {
    let start = Instant::now();
    let mut detector = BlackBarDetector::new();
    let boxed = picture([48, 0, 48, 0], |_, _| 128);
    let full = picture([0; 4], |_, _| 128);
    for i in 0..=50 {
        let frame = if i % 2 == 0 { &boxed } else { &full };
        assert_eq!(at(&mut detector, frame, start, i * 100), viewport([0; 4]));
    }
}

#[test]
fn low_luma_colored_picture_is_not_a_neutral_black_bar() {
    let mut bytes = Vec::with_capacity((WIDTH * HEIGHT * 2) as usize);
    for y in 0..HEIGHT {
        for _ in (0..WIDTH).step_by(2) {
            let (luma, chroma) = if (40..320).contains(&y) {
                (128, 128)
            } else {
                (18, 190)
            };
            bytes.extend_from_slice(&[luma, 128, luma, chroma]);
        }
    }
    let frame = Frame::new(
        bytes,
        WIDTH,
        HEIGHT,
        PixelFormat::YUYV {
            range: YuvRange::Limited,
            matrix: YuvStandardMatrix::Bt709,
        },
    )
    .unwrap();
    assert_eq!(
        feed(
            &mut BlackBarDetector::new(),
            &frame,
            Instant::now(),
            0,
            2000
        ),
        viewport([0; 4])
    );
}

#[test]
fn rgb_and_bgr_frames_detect_the_same_asymmetric_geometry() {
    for format in [PixelFormat::RGB24, PixelFormat::BGR24] {
        let mut bytes = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
        for y in 0..HEIGHT {
            for _ in 0..WIDTH {
                bytes.extend_from_slice(if (20..350).contains(&y) {
                    &[80, 120, 160]
                } else {
                    &[0, 0, 0]
                });
            }
        }
        let frame = Frame::new(bytes, WIDTH, HEIGHT, format).unwrap();
        assert_eq!(
            feed(
                &mut BlackBarDetector::new(),
                &frame,
                Instant::now(),
                0,
                2000
            ),
            viewport([20, 0, 10, 0])
        );
    }
}

#[test]
fn sparse_sustained_picture_detail_vetoes_a_deeper_crop() {
    let start = Instant::now();
    let mut detector = BlackBarDetector::new();
    let clean = picture([20, 0, 20, 0], |_, _| 128);
    feed(&mut detector, &clean, start, 0, 2000);
    let dim = picture([20, 0, 20, 0], |x, y| {
        if y >= 80 {
            128
        } else if (308..332).contains(&x) {
            12
        } else {
            0
        }
    });
    for millis in (2050..=12_050).step_by(50) {
        let crop = at(&mut detector, &dim, start, millis);
        assert!(
            crop.y <= 20,
            "sparse picture detail was cropped at {millis}ms: {crop:?}"
        );
    }
}

#[test]
fn noisy_bars_and_soft_boundary_pixels_stay_within_the_locked_geometry() {
    let start = Instant::now();
    let mut detector = BlackBarDetector::new();
    let clean = picture([20, 0, 11, 0], |_, _| 128);
    let expected = viewport([20, 0, 11, 0]);
    assert_eq!(feed(&mut detector, &clean, start, 0, 2000), expected);
    for i in 0..100 {
        let frame = yuyv_frame_from_luma(WIDTH, HEIGHT, |x, y| match y {
            20 => 6 + u8::try_from(i % 9).unwrap(),
            349 => 6 + u8::try_from((i * 3) % 9).unwrap(),
            21..=348 => 128,
            _ => u8::try_from((x + y + i) % 3).unwrap(),
        });
        assert_eq!(
            at(&mut detector, &frame, start, 2050 + u64::from(i) * 100),
            expected,
            "compression noise/blur moved confirmed crop, phase {i}"
        );
    }
}
