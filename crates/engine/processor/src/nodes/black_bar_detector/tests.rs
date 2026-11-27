use super::{BlackBarDetector, Candidate, EdgeCrop, Px, Sighting};
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
    let mut detector = BlackBarDetector::default();
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
                &mut BlackBarDetector::default(),
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
        let mut detector = BlackBarDetector::default();
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
        let mut detector = BlackBarDetector::default();
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
    let mut detector = BlackBarDetector::default();
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
    let mut candidate = BlackBarDetector::default();
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
        let mut detector = BlackBarDetector::default();
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
    let mut detector = BlackBarDetector::default();
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
    let mut detector = BlackBarDetector::default();
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
                &mut BlackBarDetector::default(),
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
    let mut detector = BlackBarDetector::default();
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
            &mut BlackBarDetector::default(),
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
                &mut BlackBarDetector::default(),
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
    let mut detector = BlackBarDetector::default();
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
    let mut detector = BlackBarDetector::default();
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

/// The same picture as NV12: luma copied, chroma from the even rows.
fn as_nv12(yuyv: &Frame) -> Frame {
    let PixelFormat::YUYV { range, matrix } = yuyv.format() else {
        panic!("expected YUYV");
    };
    let data = yuyv.as_slice();
    let mut bytes: Vec<u8> = data.iter().step_by(2).copied().collect();
    for row in data
        .as_chunks::<{ WIDTH as usize * 2 }>()
        .0
        .iter()
        .step_by(2)
    {
        for &[_, u, _, v] in row.as_chunks::<4>().0 {
            bytes.extend_from_slice(&[u, v]);
        }
    }
    Frame::new(bytes, WIDTH, HEIGHT, PixelFormat::NV12 { range, matrix }).unwrap()
}

#[test]
fn nv12_frames_detect_the_same_bars_as_yuyv_and_see_their_chroma() {
    for range in [YuvRange::Full, YuvRange::Limited] {
        let bars = [30, 8, 46, 60];
        let yuyv = with_range(&picture(bars, |_, _| 128), range);
        let nv12 = as_nv12(&yuyv);
        let start = Instant::now();
        let expected = feed(&mut BlackBarDetector::default(), &yuyv, start, 0, 2000);
        assert_eq!(expected, viewport(bars), "{range:?}");
        assert_eq!(
            feed(&mut BlackBarDetector::default(), &nv12, start, 0, 2000),
            expected,
            "{range:?}"
        );
    }
    let mut bytes = Vec::with_capacity((WIDTH * HEIGHT * 3 / 2) as usize);
    let dark = |y: u32| !(40..320).contains(&y);
    for y in 0..HEIGHT {
        bytes.extend((0..WIDTH).map(|_| if dark(y) { 18 } else { 128 }));
    }
    for y in (0..HEIGHT).step_by(2) {
        for _ in (0..WIDTH).step_by(2) {
            bytes.extend_from_slice(if dark(y) { &[128, 190] } else { &[128, 128] });
        }
    }
    let frame = Frame::new(
        bytes,
        WIDTH,
        HEIGHT,
        PixelFormat::NV12 {
            range: YuvRange::Limited,
            matrix: YuvStandardMatrix::Bt709,
        },
    )
    .unwrap();
    assert_eq!(
        feed(
            &mut BlackBarDetector::default(),
            &frame,
            Instant::now(),
            0,
            2000
        ),
        viewport([0; 4])
    );
}

const SIZES: [(u32, u32); 4] = [(640, 360), (960, 540), (1280, 720), (1920, 1080)];

fn noise(x: u32, y: u32, frame: u32) -> u32 {
    let mut h =
        x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77) ^ frame.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^ (h >> 13)
}

/// One limited-range NV12 frame of a 320x180 grid of cells, shown at
/// `width` x `height` so that every size shows the same cells, plus up to 3
/// levels of noise per pixel. `cell(cx, cy)` is the luma of a cell.
fn cells(width: u32, height: u32, frame: u32, cell: impl Fn(u32, u32) -> u8) -> Frame {
    let mut bytes = Vec::with_capacity((width * height * 3 / 2) as usize);
    for y in 0..height {
        for x in 0..width {
            let level = cell(x * 320 / width, y * 180 / height);
            let grain = u8::try_from(noise(x, y, frame + 1000) % 4).unwrap();
            bytes.push(if level <= 16 { 16 } else { level + grain });
        }
    }
    bytes.resize((width * height * 3 / 2) as usize, 128);
    Frame::new(
        bytes,
        width,
        height,
        PixelFormat::NV12 {
            range: YuvRange::Limited,
            matrix: YuvStandardMatrix::Bt709,
        },
    )
    .unwrap()
}

/// A dark, noisy picture, 10 levels above black on average, with a few small
/// highlights. Letterboxed, it has 2.39:1 bars (23 of 180 cells, 138 rows at
/// 1080p) at the top and bottom, and thin near-black lines across the picture.
/// Otherwise it has no bars but darkens gradually toward every border.
fn dark_scene(width: u32, height: u32, frame: u32, letterboxed: bool) -> Frame {
    cells(width, height, frame, |cx, cy| {
        if letterboxed && !(23..157).contains(&cy) {
            return 16;
        }
        if noise(cx, cy, 0).is_multiple_of(16) {
            return 90;
        }
        if letterboxed && cy % 6 == 3 {
            return 17;
        }
        let border = cx.min(319 - cx).min(cy.min(179 - cy) * 16 / 9);
        let vignette = if letterboxed { 2 } else { border.min(24) / 3 };
        18 + u8::try_from(vignette + noise(cx, cy, frame) % 9).unwrap()
    })
}

/// The crop (top, right, bottom, left) of every scan from 0 to `seconds` at 20
/// scans/s, cycling through `frames`.
fn crops(frames: &[Frame], seconds: usize) -> Vec<[u32; 4]> {
    let start = Instant::now();
    let mut detector = BlackBarDetector::default();
    (0..seconds * 20)
        .map(|scan| {
            let frame = &frames[scan % frames.len()];
            let crop = at(&mut detector, frame, start, scan as u64 * 50);
            [
                crop.y,
                frame.width() - crop.x - crop.width,
                frame.height() - crop.y - crop.height,
                crop.x,
            ]
        })
        .collect()
}

fn changes(crops: &[[u32; 4]]) -> usize {
    crops.windows(2).filter(|pair| pair[0] != pair[1]).count()
}

/// The 23-cell bars of [`dark_scene`] within one lattice step (3 rows at 1080p).
fn is_bar(rows: u32, height: u32) -> bool {
    rows.abs_diff(23 * height / 180) <= (height / 360).max(1)
}

#[test]
fn a_dark_letterboxed_scene_is_cropped_alike_and_once_at_every_capture_size() {
    for (width, height) in SIZES {
        let frames: Vec<_> = (0..8).map(|i| dark_scene(width, height, i, true)).collect();
        let crops = crops(&frames, 6);
        let first = crops.iter().position(|crop| crop[0] > 0).unwrap();
        assert!(
            (30..=40).contains(&first),
            "{width}x{height}: cropped after scan {first}"
        );
        assert_eq!(changes(&crops), 1, "{width}x{height}: the crop flickered");
        let [top, right, bottom, left] = *crops.last().unwrap();
        assert!(
            is_bar(top, height) && is_bar(bottom, height) && right == 0 && left == 0,
            "{width}x{height}: {:?}",
            crops.last()
        );
    }
}

#[test]
fn a_dark_scene_without_bars_is_never_cropped_at_any_capture_size() {
    for (width, height) in SIZES {
        let frames: Vec<_> = (0..8)
            .map(|i| dark_scene(width, height, i, false))
            .collect();
        assert!(
            crops(&frames, 6).iter().all(|crop| *crop == [0; 4]),
            "{width}x{height}: dark borders were cropped"
        );
    }
}

#[test]
fn intermittent_sightings_in_a_dark_scene_crop_after_enough_evidence_without_flicker() {
    for (width, height) in SIZES {
        let black = cells(width, height, 0, |_, _| 16);
        let mut frames: Vec<_> = (0..8).map(|i| dark_scene(width, height, i, true)).collect();
        frames.insert(2, black.clone());
        frames.insert(5, black.clone());
        frames.insert(8, black);
        let crops = crops(&frames, 8);
        let first = crops.iter().position(|crop| crop[0] > 0).unwrap();
        assert!(
            (40..=60).contains(&first),
            "{width}x{height}: cropped after scan {first}"
        );
        assert_eq!(changes(&crops), 1, "{width}x{height}: the crop flickered");
    }
    let lit = picture([48, 0, 48, 0], |_, _| 128);
    let dark = yuyv_frame_from_luma(WIDTH, HEIGHT, |_, _| 0);
    let start = Instant::now();
    let mut detector = BlackBarDetector::default();
    for scan in 0..400 {
        let frame = if scan % 3 == 0 { &lit } else { &dark };
        assert_eq!(at(&mut detector, frame, start, scan * 50), viewport([0; 4]));
    }
}

#[test]
fn lit_letterbox_bars_are_found_in_1_5_seconds_at_every_capture_size() {
    for (width, height) in SIZES {
        let frames = [cells(width, height, 0, |cx, cy| {
            if (23..157).contains(&cy) {
                60 + u8::try_from(noise(cx, cy, 0) % 120).unwrap()
            } else {
                16
            }
        })];
        let crops = crops(&frames, 3);
        let first = crops.iter().position(|crop| crop[0] > 0).unwrap();
        assert_eq!(first, 30, "{width}x{height}: found after scan {first}");
        let [top, _, bottom, _] = *crops.last().unwrap();
        assert!(
            is_bar(top, height) && is_bar(bottom, height),
            "{width}x{height}: {:?}",
            crops.last()
        );
    }
}

/// A portrait frame with a lit band 45 rows high: top and bottom both crop to
/// it after 1.5 s, which together leave less than a tenth of the height.
fn band() -> Frame {
    yuyv_frame_from_luma(
        360,
        640,
        |_, y| {
            if (290..335).contains(&y) { 128 } else { 0 }
        },
    )
}

#[test]
fn a_rejected_crop_keeps_the_evidence_of_the_edges_that_did_not_move() {
    let start = Instant::now() + Duration::from_secs(10);
    let mut detector = BlackBarDetector::default();
    detector.edges[3].candidate = Some(Candidate {
        anchor: Px(40),
        seen: (0..20_u64)
            .map(|i| Sighting {
                time: start - Duration::from_millis(1000 - i * 50),
                credit: Duration::from_millis(if i == 0 { 0 } else { 50 }),
                position: Px(40),
            })
            .collect(),
    });
    let frame = band();
    for millis in (0..=1500).step_by(50) {
        assert_eq!(
            at(&mut detector, &frame, start, millis),
            Viewport::full_frame(360, 640),
            "{millis} ms"
        );
    }
    let edges = &detector.edges;
    assert!(edges[0].candidate.is_none() && edges[2].candidate.is_none());
    assert_eq!(detector.crops(), [Px(0); 4]);
    assert!(
        edges[3].candidate.is_some(),
        "the left edge lost its evidence"
    );
}

/// An edge's state for [`one_edge_proposes_once_its_evidence_confirms`].
#[derive(Debug, Clone, Copy)]
enum State {
    /// At its crop, 40 px, with no candidate.
    Idle,
    /// A candidate 80 px in, seen for 0.5 s.
    Gathering,
    /// A candidate 80 px in, one scan short of 1.5 s.
    Due,
    /// As `Due`, but a scan 0.1 s ago saw the outer end of the candidate’s jitter.
    DueVetoed,
    /// At 80 px, with a candidate 40 px in one scan short of 0.4 s.
    DueOutward,
}

/// What one scan, 50 ms after the previous, sees at an edge.
#[derive(Debug, Clone, Copy)]
enum Scan {
    Unclear,
    Current,
    Candidate,
    Other,
}

const SCAN: Duration = Duration::from_millis(50);

/// An edge in `state` at `now`: the candidate's sightings are one scan apart,
/// the last a scan before `now`.
fn edge(state: State, now: Instant) -> EdgeCrop {
    let (current, anchor, scans) = match state {
        State::Idle => (40, None, 0),
        State::Gathering => (40, Some(80), 11),
        State::Due | State::DueVetoed => (40, Some(80), 30),
        State::DueOutward => (80, Some(40), 8),
    };
    let mut edge = EdgeCrop {
        current: Px(current),
        ..EdgeCrop::default()
    };
    for i in 0..scans {
        let position = match state {
            State::DueVetoed if i == scans - 2 => 76,
            State::DueVetoed if i > 0 => 84,
            _ => anchor.expect("an edge with sightings has a candidate"),
        };
        let at = now - SCAN * (scans - i);
        assert_eq!(edge.propose(Some(Px(position)), at, SCAN, Px(4)), None);
    }
    edge
}

#[test]
fn one_edge_proposes_once_its_evidence_confirms() {
    use Scan::{Candidate, Current, Other, Unclear};
    use State::{Due, DueOutward, DueVetoed, Gathering, Idle};
    // State, scan, then the proposal and the candidate's anchor after it.
    let table = [
        (Idle, Unclear, None, None),
        (Idle, Current, None, None),
        (Idle, Candidate, None, Some(80)),
        (Idle, Other, None, Some(120)),
        (Gathering, Unclear, None, Some(80)),
        (Gathering, Current, None, None),
        (Gathering, Candidate, None, Some(80)),
        (Gathering, Other, None, Some(120)),
        (Due, Unclear, None, Some(80)),
        (Due, Current, None, None),
        (Due, Candidate, Some(80), Some(80)),
        (Due, Other, None, Some(120)),
        (DueVetoed, Unclear, None, Some(80)),
        (DueVetoed, Current, None, None),
        (DueVetoed, Candidate, None, Some(80)),
        (DueVetoed, Other, None, Some(120)),
        (DueOutward, Unclear, None, Some(40)),
        (DueOutward, Current, None, None),
        (DueOutward, Candidate, Some(40), Some(40)),
        (DueOutward, Other, None, Some(120)),
    ];
    let now = Instant::now() + Duration::from_secs(10);
    for (state, scan, proposal, anchor) in table {
        let mut edge = edge(state, now);
        let current = edge.current;
        let candidate = edge.candidate.as_ref().map(|candidate| candidate.anchor);
        let observed = match scan {
            Unclear => None,
            Current => Some(current),
            Candidate => Some(candidate.unwrap_or(Px(80))),
            Other => Some(Px(120)),
        };
        let proposed = edge.propose(observed, now, SCAN, Px(4));
        assert_eq!(
            (
                proposed,
                edge.candidate.as_ref().map(|candidate| candidate.anchor)
            ),
            (proposal.map(Px), anchor.map(Px)),
            "{state:?} after {scan:?}"
        );
        assert_eq!(edge.current, current, "only a commit moves the crop");
    }
}

#[test]
fn an_inward_veto_expires_at_the_confirmation_boundary() {
    let start = Instant::now();
    let mut edge = EdgeCrop {
        current: Px(40),
        ..EdgeCrop::default()
    };
    // All sightings stay within 4 px of the anchor. The low one rules out
    // the median of 84 until it is exactly 1.5 s old.
    for scan in 0..=30 {
        let position = match scan {
            0 => 80,
            1 => 76,
            _ => 84,
        };
        assert_eq!(
            edge.propose(Some(Px(position)), start + SCAN * scan, SCAN, Px(4)),
            None
        );
    }
    assert_eq!(
        edge.propose(Some(Px(84)), start + SCAN * 31, SCAN, Px(4)),
        Some(Px(84))
    );
}

#[test]
fn proposals_commit_together_only_when_they_leave_a_tenth_of_the_frame() {
    let frame = Frame::dummy(WIDTH, HEIGHT).unwrap();
    let now = Instant::now() + Duration::from_secs(10);
    let some = |crops: [u32; 4]| crops.map(|crop| (crop > 0).then_some(Px(crop)));
    // Proposals (top, right, bottom, left; 0 none), then the crops after them.
    let table = [
        ([0; 4], [0; 4]),
        ([20, 0, 20, 0], [20, 0, 20, 0]),
        ([0, 300, 0, 276], [0, 300, 0, 276]),
        ([200, 0, 150, 0], [0; 4]),
        ([330, 0, 0, 0], [0; 4]),
        ([0, 300, 0, 300], [0; 4]),
    ];
    for (proposals, crops) in table {
        let mut detector = BlackBarDetector::default();
        for crop in &mut detector.edges {
            *crop = edge(State::Gathering, now);
            crop.current = Px(0);
        }
        detector.commit(&frame, some(proposals));
        assert_eq!(detector.crops(), crops.map(Px), "{proposals:?}");
        for (edge, proposal) in detector.edges.iter().zip(proposals) {
            assert_eq!(
                edge.candidate.is_some(),
                proposal == 0,
                "{proposals:?}: a proposal uses up its evidence, others keep theirs"
            );
        }
    }
}
