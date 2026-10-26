use super::*;
use crate::frames::yuyv_frame_from_luma;

#[test]
fn a_missing_bar_prevents_and_releases_cropping_on_its_axis() {
    for thickness in [6, 48] {
        for missing_edge in 0..4 {
            let make_frame = |missing| {
                yuyv_frame_from_luma(640, 360, |x, y| {
                    if (missing != 0 && y < thickness)
                        || (missing != 1 && x >= 640 - thickness)
                        || (missing != 2 && y >= 360 - thickness)
                        || (missing != 3 && x < thickness)
                    {
                        0
                    } else {
                        128
                    }
                })
            };
            let unpaired = make_frame(missing_edge);
            let paired = make_frame(4);
            let horizontal = missing_edge == 0 || missing_edge == 2;
            let expected = Viewport {
                x: if horizontal { thickness } else { 0 },
                y: if horizontal { 0 } else { thickness },
                width: if horizontal { 640 - 2 * thickness } else { 640 },
                height: if horizontal { 360 } else { 360 - 2 * thickness },
            };
            let mut detector = BlackBarDetector::new();
            let start = Instant::now();
            for (frame, seconds) in [(&unpaired, 0), (&paired, 1), (&unpaired, 2)] {
                let now = start + Duration::from_secs(seconds);
                detector.process(FrameView::new(frame.clone()), now);
                let actual = detector
                    .process(FrameView::new(frame.clone()), now + CONFIRM_DURATION)
                    .viewport;
                let target = if seconds == 1 {
                    Viewport {
                        x: thickness,
                        y: thickness,
                        width: 640 - 2 * thickness,
                        height: 360 - 2 * thickness,
                    }
                } else {
                    expected
                };
                assert_eq!(
                    actual, target,
                    "missing edge {missing_edge}, thickness {thickness}, phase {seconds}"
                );
            }
        }
    }
}

#[test]
fn dark_foreground_does_not_extend_one_bar_into_the_picture() {
    let frame = create_letterboxed_frame(640, 360, 6, 100);
    let (horizontal, _) = BlackBarDetector::new().detect_bands(&frame);
    assert_eq!(horizontal, Some((6, 6)));
}

#[test]
fn dark_scene_regions_with_visible_content_are_not_black_bars() {
    for edge in 0..4 {
        let frame = yuyv_frame_from_luma(640, 360, |x, y| {
            let (distance, cross, center) = match edge {
                0 => (y, x, 320),
                1 => (639 - x, y, 180),
                2 => (359 - y, x, 320),
                _ => (x, y, 180),
            };
            if distance >= 120 {
                128
            } else if (20..80).contains(&distance) && cross.abs_diff(center) < 12 {
                12 // Dim detail occupies less than 5% of the row or column.
            } else {
                0
            }
        });
        let mut detector = BlackBarDetector::new();
        let start = Instant::now();
        detector.process(FrameView::new(frame.clone()), start);
        let viewport = detector
            .process(FrameView::new(frame), start + CONFIRM_DURATION)
            .viewport;
        let removed = match edge {
            0 => viewport.y,
            1 => 640 - viewport.x - viewport.width,
            2 => 360 - viewport.y - viewport.height,
            _ => viewport.x,
        };
        assert!(
            removed <= 20,
            "edge {edge} cropped visible detail: {viewport:?}"
        );
    }
}

#[test]
fn isolated_highlights_do_not_seed_a_crop() {
    for cross in [false, true] {
        let frame = yuyv_frame_from_luma(640, 360, |x, y| {
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
        assert_eq!(BlackBarDetector::new().detect_bands(&frame), (None, None));
    }
}

#[test]
fn confirmation_and_release_use_elapsed_time() {
    let start = Instant::now();
    let boxed = create_letterboxed_frame(640, 360, 48, 48);
    let full = yuyv_frame_from_luma(640, 360, |_, _| 128);
    for fps in [24, 30, 60, 120] {
        let mut detector = BlackBarDetector::new();
        for (frame, offset, before, after) in [
            (&boxed, Duration::ZERO, 0, 48),
            (&full, Duration::from_secs(2), 48, 0),
        ] {
            let mut first_change = None;
            for i in 0..=fps {
                let elapsed = Duration::from_secs_f64(f64::from(i) / f64::from(fps));
                let output =
                    detector.process(FrameView::new(frame.clone()), start + offset + elapsed);
                if elapsed < CONFIRM_DURATION {
                    assert_eq!(output.viewport.y, before, "{fps} fps confirmed too early");
                }
                if output.viewport.y == after && first_change.is_none() {
                    first_change = Some(elapsed);
                }
            }
            let elapsed = first_change.expect("crop must converge within one second");
            let frame_period = Duration::from_secs_f64(1.0 / f64::from(fps));
            assert!(
                elapsed <= CONFIRM_DURATION + SCAN_INTERVAL + frame_period,
                "{fps} fps took {elapsed:?} to converge"
            );
        }
    }
    let mut detector = BlackBarDetector::new();
    for millis in [0, 70, 150, 230] {
        let output = detector.process(
            FrameView::new(boxed.clone()),
            start + Duration::from_millis(millis),
        );
        assert_eq!(output.viewport.y, if millis >= 200 { 48 } else { 0 });
    }
}

#[test]
fn repeated_or_earlier_times_do_not_advance_or_reset_detection() {
    let start = Instant::now();
    let mut detector = BlackBarDetector::new();
    let boxed = create_letterboxed_frame(640, 360, 48, 48);
    let full = yuyv_frame_from_luma(640, 360, |_, _| 128);
    for millis in [0, 100] {
        detector.process(
            FrameView::new(boxed.clone()),
            start + Duration::from_millis(millis),
        );
    }
    for now in [
        start.checked_sub(Duration::from_secs(1)).unwrap(),
        start,
        start + Duration::from_millis(100),
    ] {
        assert_eq!(
            detector
                .process(FrameView::new(full.clone()), now)
                .viewport
                .y,
            0
        );
    }
    assert_eq!(
        detector
            .process(FrameView::new(boxed), start + CONFIRM_DURATION)
            .viewport
            .y,
        48
    );
}

#[test]
fn darkness_preserves_crop_but_cannot_confirm_a_candidate() {
    let start = Instant::now();
    let mut detector = BlackBarDetector::new();
    let boxed = create_letterboxed_frame(640, 360, 48, 48);
    let dark = yuyv_frame_from_luma(640, 360, |_, _| 20);
    for (frame, millis, expected_y) in [
        (&boxed, 0, 0),
        (&dark, 500, 0),
        (&boxed, 1100, 0),
        (&boxed, 2100, 48),
        (&dark, 2200, 48),
        (&dark, 30200, 48),
    ] {
        let output = detector.process(
            FrameView::new(frame.clone()),
            start + Duration::from_millis(millis),
        );
        assert_eq!(output.viewport.y, expected_y);
    }
}

#[test]
fn alternating_scans_do_not_confirm_a_crop() {
    let start = Instant::now();
    let mut detector = BlackBarDetector::new();
    let boxed = create_letterboxed_frame(640, 360, 48, 48);
    let full = yuyv_frame_from_luma(640, 360, |_, _| 128);
    for i in 0..=20 {
        let frame = if i % 2 == 0 { &boxed } else { &full };
        assert_eq!(
            detector
                .process(
                    FrameView::new(frame.clone()),
                    start + Duration::from_millis(i * 100)
                )
                .viewport
                .y,
            0
        );
    }
    let output = detector.process(FrameView::new(boxed), start + Duration::from_millis(3100));
    assert_eq!(output.viewport.y, 48);
}

#[test]
fn stable_side_bars_confirm_while_subtitles_change_bottom_boundary() {
    let start = Instant::now();
    let mut detector = BlackBarDetector::new();
    for i in 0..=10 {
        let frame = yuyv_frame_from_luma(640, 360, |x, y| {
            if (80..560).contains(&x) && (48..312).contains(&y) {
                128
            } else if i % 2 == 1 && (160..480).contains(&x) && y == 330 {
                255
            } else {
                0
            }
        });
        let output = detector.process(
            FrameView::new(frame),
            start + Duration::from_millis(i * 100),
        );
        assert_eq!(output.viewport.y, 0);
        assert_eq!(output.viewport.x, if i >= 2 { 80 } else { 0 });
    }
    let frame = yuyv_frame_from_luma(640, 360, |x, y| {
        if (80..560).contains(&x) && (48..312).contains(&y) {
            128
        } else {
            0
        }
    });
    let output = detector.process(FrameView::new(frame), start + Duration::from_secs(2));
    assert_eq!(
        output.viewport,
        Viewport {
            x: 80,
            y: 48,
            width: 480,
            height: 264
        }
    );
}

#[test]
fn boundary_jitter_does_not_move_confirmed_crop_or_hide_thin_bars() {
    let start = Instant::now();
    let mut axis = AxisCrop::default();
    axis.update(Some((2, 2)), start);
    axis.update(Some((2, 2)), start + CONFIRM_DURATION);
    assert_eq!(axis.current, (2, 2));
    for seconds in 2..10 {
        axis.update(Some((3, 3)), start + Duration::from_secs(seconds));
        assert_eq!(axis.current, (2, 2));
    }
    axis.update(Some((0, 0)), start + Duration::from_secs(10));
    axis.update(Some((0, 0)), start + Duration::from_secs(11));
    assert_eq!(axis.current, (0, 0));
}

/// Create test frame with letterboxing (black bands top/bottom, gray content)
fn create_letterboxed_frame(width: u32, height: u32, top_band: u32, bottom_band: u32) -> Frame {
    yuyv_frame_from_luma(width, height, |_, y| {
        if y < top_band || y >= height - bottom_band {
            0
        } else {
            128
        }
    })
}

/// Create test frame with pillarboxing (black bands left/right, gray content)
fn create_pillarboxed_frame(width: u32, height: u32, left_band: u32, right_band: u32) -> Frame {
    yuyv_frame_from_luma(width, height, |x, _| {
        if x < left_band || x >= width - right_band {
            0
        } else {
            128
        }
    })
}

/// Create test frame with subtitle in the bottom black band
fn create_frame_with_subtitles(width: u32, height: u32, subtitle_row: u32) -> Frame {
    yuyv_frame_from_luma(width, height, |x, y| {
        if y == subtitle_row && x >= width / 4 && x < 3 * width / 4 {
            255 // Subtitle text
        } else if y >= 100 && y < height - 100 {
            128 // Content
        } else {
            0 // Black bands
        }
    })
}

#[test]
fn sample_percentile_handles_median_bright_tail_and_empty_input() {
    assert_eq!(percentile(&mut [0, 100, 200], 50), 100);

    let even = percentile(&mut [0, 100, 200, 255], 50);
    assert!(even == 100 || even == 200); // Either is valid for even length

    assert_eq!(percentile(&mut [], 90), 0);
    assert_eq!(percentile(&mut [128], 50), 128);
    assert_eq!(percentile(&mut [0, 0, 0, 0, 0, 0, 0, 0, 128, 255], 90), 255);
}

#[test]
fn test_band_boundary_detection() {
    // Continuous black region larger than minimum
    let mut brightness = vec![0; 50];
    brightness.extend([100, 100, 100]);
    assert_eq!(find_band_from_start(&brightness, 10, 50), 50);

    // Continuous black region smaller than minimum
    let brightness = vec![0, 0, 0, 0, 0, 100, 100, 100];
    assert_eq!(find_band_from_start(&brightness, 10, 50), 0);

    // Discontinuous (subtitle case)
    let brightness = vec![0, 0, 0, 0, 200, 0, 0, 0]; // White pixel breaks continuity
    assert_eq!(find_band_from_start(&brightness, 10, 50), 0);

    // From end - need at least 50 zeros for minimum band size
    let mut brightness = vec![100, 100, 100];
    brightness.extend(vec![0; 50]);
    assert_eq!(find_band_from_end(&brightness, 10, 50), 50);
}

#[test]
fn test_letterbox_detection() {
    let mut detector = BlackBarDetector::new();

    // Create 1920x1080 frame with 240px bands top/bottom (2.35:1 aspect ratio)
    let frame = create_letterboxed_frame(1920, 1080, 240, 240);

    let (horizontal, vertical) = detector.detect_bands(&frame);
    let (top, bottom) = horizontal.unwrap();
    let (left, right) = vertical.unwrap();
    let viewport = Viewport {
        x: left,
        y: top,
        width: frame.width - left - right,
        height: frame.height - top - bottom,
    };

    // Should detect both bands (allowing some tolerance)
    assert!(
        viewport.y >= 230 && viewport.y <= 250,
        "Top band: {}",
        viewport.y
    );
    assert!(
        viewport.height >= 580 && viewport.height <= 620,
        "Height: {}",
        viewport.height
    );
    assert_eq!(viewport.x, 0);
    assert_eq!(viewport.width, 1920);
}

#[test]
fn test_letterbox_detection_low_res() {
    let mut detector = BlackBarDetector::new();

    // 640x360 capture with 2.35:1 content: 48px bands top/bottom
    let frame = create_letterboxed_frame(640, 360, 48, 48);

    let (horizontal, vertical) = detector.detect_bands(&frame);
    let (top, bottom) = horizontal.unwrap();
    let (left, right) = vertical.unwrap();
    let viewport = Viewport {
        x: left,
        y: top,
        width: frame.width - left - right,
        height: frame.height - top - bottom,
    };

    assert!(
        viewport.y >= 44 && viewport.y <= 52,
        "Top band: {}",
        viewport.y
    );
    assert!(
        viewport.height >= 256 && viewport.height <= 272,
        "Height: {}",
        viewport.height
    );
    assert_eq!(viewport.x, 0);
    assert_eq!(viewport.width, 640);
}

#[test]
fn test_pillarbox_detection() {
    let mut detector = BlackBarDetector::new();

    // Create frame with 240px bands left/right
    let frame = create_pillarboxed_frame(1920, 1080, 240, 240);

    let (horizontal, vertical) = detector.detect_bands(&frame);
    let (top, bottom) = horizontal.unwrap();
    let (left, right) = vertical.unwrap();
    let viewport = Viewport {
        x: left,
        y: top,
        width: frame.width - left - right,
        height: frame.height - top - bottom,
    };

    // Should detect both bands
    assert!(
        viewport.x >= 230 && viewport.x <= 250,
        "Left band: {}",
        viewport.x
    );
    assert!(
        viewport.width >= 1420 && viewport.width <= 1460,
        "Width: {}",
        viewport.width
    );
    assert_eq!(viewport.y, 0);
    assert_eq!(viewport.height, 1080);
}

#[test]
fn test_subtitle_handling() {
    let mut detector = BlackBarDetector::new();

    // Frame with subtitle in bottom black band
    let frame = create_frame_with_subtitles(1920, 1080, 1000);

    let (horizontal, vertical) = detector.detect_bands(&frame);
    let (top, bottom) = horizontal.unwrap();
    let (left, right) = vertical.unwrap();
    let viewport = Viewport {
        x: left,
        y: top,
        width: frame.width - left - right,
        height: frame.height - top - bottom,
    };

    // Should still detect top band correctly
    assert!(
        viewport.y >= 90 && viewport.y <= 110,
        "Top band: {}",
        viewport.y
    );

    // Bottom band detection should stop at subtitle or detect reduced band
    // The exact behavior depends on subtitle density, but should not crash
    assert!(viewport.height > 0 && viewport.height <= 980);
}

#[test]
fn test_no_letterbox() {
    let mut detector = BlackBarDetector::new();

    // Full frame of gray content
    let frame = yuyv_frame_from_luma(1920, 1080, |_, _| 128);

    let (horizontal, vertical) = detector.detect_bands(&frame);
    let (top, bottom) = horizontal.unwrap();
    let (left, right) = vertical.unwrap();
    let viewport = Viewport {
        x: left,
        y: top,
        width: frame.width - left - right,
        height: frame.height - top - bottom,
    };

    // Should detect no bands
    assert_eq!(viewport.x, 0);
    assert_eq!(viewport.y, 0);
    assert_eq!(viewport.width, 1920);
    assert_eq!(viewport.height, 1080);
}

#[test]
fn test_projection_building() {
    let mut detector = BlackBarDetector::new();

    let frame = create_letterboxed_frame(1920, 1080, 100, 100);

    let row_projection = detector.build_row_projection(&frame, 0..frame.width, 50);
    assert_eq!(row_projection.len(), 1080);

    // Top 100 rows should be black (brightness ~0)
    for i in 0..100 {
        assert!(
            row_projection[i] < 20,
            "Row {} brightness: {}",
            i,
            row_projection[i]
        );
    }

    // Middle rows should be gray (brightness ~128)
    for i in 100..980 {
        assert!(
            row_projection[i] > 100,
            "Row {} brightness: {}",
            i,
            row_projection[i]
        );
    }

    // Bottom 100 rows should be black
    for i in 980..1080 {
        assert!(
            row_projection[i] < 20,
            "Row {} brightness: {}",
            i,
            row_projection[i]
        );
    }
}
