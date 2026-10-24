use crate::frames::{ViewFrame, Viewport, luma_at};
use prysm_capture::{Frame, PixelFormat, YuvRange};
use std::ops::Range;
use std::time::{Duration, Instant};

const SCAN_INTERVAL: Duration = Duration::from_millis(100);
const CONFIRM_DURATION: Duration = Duration::from_secs(1);
const SAMPLE_STRIDE: usize = 8;

type Bands = (u32, u32);

/// Detects letterboxing and pillarboxing directly from raw frame luma.
#[derive(Debug, Default)]
pub struct BandDetector {
    horizontal: AxisCrop,
    vertical: AxisCrop,
    last_scan: Option<Instant>,
    sample_scratch: Vec<u8>,
    grid_scratch: Vec<u8>,
}

/// Each axis confirms independently so subtitles cannot delay stable side bars.
#[derive(Debug, Default)]
struct AxisCrop {
    current: Bands,
    candidate: Option<(Bands, Instant)>,
}

impl AxisCrop {
    fn update(&mut self, detected: Option<Bands>, now: Instant) {
        let Some(detected) = detected else {
            // Darkness is not evidence for either a new crop or its removal.
            self.candidate = None;
            return;
        };
        if bands_match(detected, self.current) {
            self.candidate = None;
            return;
        }
        match self.candidate {
            Some((candidate, since)) if bands_match(candidate, detected) => {
                if now.saturating_duration_since(since) >= CONFIRM_DURATION {
                    self.current = candidate;
                    self.candidate = None;
                }
            }
            _ => self.candidate = Some((detected, now)),
        }
    }
}

fn bands_match(a: Bands, b: Bands) -> bool {
    a.0.abs_diff(b.0) <= 2
        && a.1.abs_diff(b.1) <= 2
        && (a.0 == 0) == (b.0 == 0)
        && (a.1 == 0) == (b.1 == 0)
}

impl BandDetector {
    pub fn new() -> Self {
        Self::default()
    }

    fn detect_bands(&mut self, frame: &Frame) -> (Option<Bands>, Option<Bands>) {
        // Raw limited-range Y has a black level of 16; RGB/full-range Y starts at 0.
        let threshold = if frame.format == PixelFormat::YUYV && frame.yuv_range == YuvRange::Limited
        {
            23
        } else {
            8
        };
        let min_row_band = (frame.height / 200).max(2);
        let min_col_band = (frame.width / 200).max(2);
        let rows = self.build_row_projection(frame, 0..frame.width, 50);
        let cols = self.build_col_projection(frame, 0..frame.height, 50);
        let mut horizontal = detect_axis(&rows, threshold, min_row_band);
        let mut vertical = detect_axis(&cols, threshold, min_col_band);

        if horizontal.is_none() && vertical.is_none() && frame.width > 0 && frame.height > 0 {
            // Look across the frame so a dark center or bright inner patch cannot choose the crop.
            let rows = self.build_row_projection(frame, 0..frame.width, 90);
            let cols = self.build_col_projection(frame, 0..frame.height, 90);
            if let (Some(h), Some(v)) = (
                detect_axis(&rows, threshold, min_row_band),
                detect_axis(&cols, threshold, min_col_band),
            ) {
                let rows = self.build_row_projection(frame, v.0..frame.width - v.1, 50);
                let cols = self.build_col_projection(frame, h.0..frame.height - h.1, 50);
                if let (Some(checked_h), Some(checked_v)) = (
                    detect_axis(&rows, threshold, min_row_band),
                    detect_axis(&cols, threshold, min_col_band),
                ) && bands_match(h, checked_h)
                    && bands_match(v, checked_v)
                {
                    return (Some(checked_h), Some(checked_v));
                }
            }
        }

        // Exclude the other axis's bars before taking medians of narrow content.
        if let Some((left, right)) = vertical
            && left + right > 0
        {
            let rows = self.build_row_projection(frame, left..frame.width - right, 50);
            horizontal = detect_axis(&rows, threshold, min_row_band);
        }
        if let Some((top, bottom)) = horizontal
            && top + bottom > 0
        {
            let cols = self.build_col_projection(frame, top..frame.height - bottom, 50);
            vertical = detect_axis(&cols, threshold, min_col_band);
        }
        (horizontal, vertical)
    }

    fn build_row_projection(
        &mut self,
        frame: &Frame,
        columns: Range<u32>,
        percent: usize,
    ) -> Vec<u8> {
        let mut projection = Vec::with_capacity(frame.height as usize);
        for y in 0..frame.height {
            self.sample_scratch.clear();
            for x in columns.clone().step_by(SAMPLE_STRIDE) {
                self.sample_scratch.push(luma_at(frame, x, y));
            }
            projection.push(percentile(&mut self.sample_scratch, percent));
        }
        projection
    }

    /// Read strided rows sequentially before taking each column's median.
    fn build_col_projection(&mut self, frame: &Frame, rows: Range<u32>, percent: usize) -> Vec<u8> {
        let width = frame.width as usize;
        let n_rows = rows.clone().step_by(SAMPLE_STRIDE).count();
        self.grid_scratch.resize(width * n_rows, 0);
        for (k, y) in rows.step_by(SAMPLE_STRIDE).enumerate() {
            for x in 0..frame.width {
                self.grid_scratch[k * width + x as usize] = luma_at(frame, x, y);
            }
        }
        let mut projection = Vec::with_capacity(width);
        for x in 0..width {
            self.sample_scratch.clear();
            for k in 0..n_rows {
                self.sample_scratch.push(self.grid_scratch[k * width + x]);
            }
            projection.push(percentile(&mut self.sample_scratch, percent));
        }
        projection
    }

    pub fn process(&mut self, mut input: ViewFrame, now: Instant) -> ViewFrame {
        if self
            .last_scan
            .is_none_or(|last| now.saturating_duration_since(last) >= SCAN_INTERVAL)
        {
            self.last_scan = Some(now);
            let (horizontal, vertical) = self.detect_bands(&input.frame);
            self.horizontal.update(horizontal, now);
            self.vertical.update(vertical, now);
        }
        let (top, bottom) = self.horizontal.current;
        let (left, right) = self.vertical.current;
        input.viewport = Viewport {
            x: left,
            y: top,
            width: input.frame.width - left - right,
            height: input.frame.height - top - bottom,
        };
        input
    }
}

fn detect_axis(brightness: &[u8], threshold: u8, min_band_size: u32) -> Option<Bands> {
    // A visible interior must contrast with the black level before a crop can change.
    if !brightness.iter().any(|&b| b >= threshold + 16) {
        return None;
    }
    let start = find_band_from_start(brightness, threshold, min_band_size);
    let end = find_band_from_end(brightness, threshold, min_band_size);
    // shortcut: require 10% picture extent to reject isolated highlights;
    // revisit this limit if tiny inset videos need support.
    if brightness.len() - start as usize - (end as usize) < brightness.len().div_ceil(10) {
        return None;
    }
    Some((start, end))
}

fn find_band_from_start(brightness: &[u8], threshold: u8, min_band_size: u32) -> u32 {
    let size = brightness
        .iter()
        .take_while(|&&b| b <= threshold)
        .fold(0, |count, _| count + 1);
    if size >= min_band_size { size } else { 0 }
}

fn find_band_from_end(brightness: &[u8], threshold: u8, min_band_size: u32) -> u32 {
    let size = brightness
        .iter()
        .rev()
        .take_while(|&&b| b <= threshold)
        .fold(0, |count, _| count + 1);
    if size >= min_band_size { size } else { 0 }
}

fn percentile(samples: &mut [u8], percent: usize) -> u8 {
    if samples.is_empty() {
        return 0;
    }
    let index = (samples.len() * percent / 100).min(samples.len() - 1);
    *samples.select_nth_unstable(index).1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::yuyv_frame_from_luma;

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
            assert_eq!(BandDetector::new().detect_bands(&frame), (None, None));
        }
    }

    #[test]
    fn confirmation_and_release_use_elapsed_time() {
        let start = Instant::now();
        let boxed = create_letterboxed_frame(640, 360, 48, 48);
        let full = yuyv_frame_from_luma(640, 360, |_, _| 128);
        for fps in [24, 30, 60, 120] {
            let mut detector = BandDetector::new();
            for i in 0..=fps {
                let now = start + Duration::from_secs_f64(f64::from(i) / f64::from(fps));
                let output = detector.process(ViewFrame::new(boxed.clone()), now);
                assert_eq!(
                    output.viewport.y,
                    if i == fps { 48 } else { 0 },
                    "{fps} fps, frame {i}"
                );
            }
            for i in 0..=fps {
                let now = start
                    + Duration::from_secs(2)
                    + Duration::from_secs_f64(f64::from(i) / f64::from(fps));
                let output = detector.process(ViewFrame::new(full.clone()), now);
                assert_eq!(
                    output.viewport.y,
                    if i == fps { 0 } else { 48 },
                    "{fps} fps, frame {i}"
                );
            }
        }
        let mut detector = BandDetector::new();
        for millis in [0, 200, 550, 900, 1100] {
            let output = detector.process(
                ViewFrame::new(boxed.clone()),
                start + Duration::from_millis(millis),
            );
            assert_eq!(output.viewport.y, if millis >= 1000 { 48 } else { 0 });
        }
    }

    #[test]
    fn repeated_or_earlier_times_do_not_advance_or_reset_detection() {
        let start = Instant::now();
        let mut detector = BandDetector::new();
        let boxed = create_letterboxed_frame(640, 360, 48, 48);
        let full = yuyv_frame_from_luma(640, 360, |_, _| 128);
        for millis in [0, 500] {
            detector.process(
                ViewFrame::new(boxed.clone()),
                start + Duration::from_millis(millis),
            );
        }
        for now in [
            start.checked_sub(Duration::from_secs(1)).unwrap(),
            start,
            start + Duration::from_millis(500),
        ] {
            assert_eq!(
                detector
                    .process(ViewFrame::new(full.clone()), now)
                    .viewport
                    .y,
                0
            );
        }
        assert_eq!(
            detector
                .process(ViewFrame::new(boxed), start + CONFIRM_DURATION)
                .viewport
                .y,
            48
        );
    }

    #[test]
    fn darkness_preserves_crop_but_cannot_confirm_a_candidate() {
        let start = Instant::now();
        let mut detector = BandDetector::new();
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
                ViewFrame::new(frame.clone()),
                start + Duration::from_millis(millis),
            );
            assert_eq!(output.viewport.y, expected_y);
        }
    }

    #[test]
    fn alternating_scans_do_not_confirm_a_crop() {
        let start = Instant::now();
        let mut detector = BandDetector::new();
        let boxed = create_letterboxed_frame(640, 360, 48, 48);
        let full = yuyv_frame_from_luma(640, 360, |_, _| 128);
        for i in 0..=20 {
            let frame = if i % 2 == 0 { &boxed } else { &full };
            assert_eq!(
                detector
                    .process(
                        ViewFrame::new(frame.clone()),
                        start + Duration::from_millis(i * 100)
                    )
                    .viewport
                    .y,
                0
            );
        }
        let output = detector.process(ViewFrame::new(boxed), start + Duration::from_millis(3100));
        assert_eq!(output.viewport.y, 48);
    }

    #[test]
    fn stable_side_bars_confirm_while_subtitles_change_bottom_boundary() {
        let start = Instant::now();
        let mut detector = BandDetector::new();
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
                ViewFrame::new(frame),
                start + Duration::from_millis(i * 100),
            );
            assert_eq!(output.viewport.y, 0);
            assert_eq!(output.viewport.x, if i == 10 { 80 } else { 0 });
        }
        let frame = yuyv_frame_from_luma(640, 360, |x, y| {
            if (80..560).contains(&x) && (48..312).contains(&y) {
                128
            } else {
                0
            }
        });
        let output = detector.process(ViewFrame::new(frame), start + Duration::from_secs(2));
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
        let mut detector = BandDetector::new();

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
        let mut detector = BandDetector::new();

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
        let mut detector = BandDetector::new();

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
        let mut detector = BandDetector::new();

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
        let mut detector = BandDetector::new();

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
        let mut detector = BandDetector::new();

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
}
