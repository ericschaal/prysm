use crate::frames::{FrameView, Viewport, luma_at};
use prysm_core::{Frame, PixelFormat, YuvRange};
use std::ops::Range;
use std::time::{Duration, Instant};

const SCAN_INTERVAL: Duration = Duration::from_millis(50);
const CONFIRM_DURATION: Duration = Duration::from_millis(200);
const SAMPLE_STRIDE: usize = 8;

type Bands = (u32, u32);

/// Detects letterboxing and pillarboxing directly from raw frame luma.
#[derive(Debug, Default)]
pub struct BlackBarDetector {
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

impl BlackBarDetector {
    pub fn new() -> Self {
        Self::default()
    }

    fn detect_bands(&mut self, frame: &Frame) -> (Option<Bands>, Option<Bands>) {
        // Raw limited-range Y has a black level of 16; RGB/full-range Y starts at 0.
        let threshold = if matches!(
            frame.format(),
            PixelFormat::YUYV {
                range: YuvRange::Limited,
                ..
            }
        ) {
            23
        } else {
            8
        };
        let min_row_band = (frame.height() / 200).max(2);
        let min_col_band = (frame.width() / 200).max(2);
        // A bar must be almost entirely black; a median can discard half the picture.
        let rows = self.build_row_projection(frame, 0..frame.width(), 98);
        let cols = self.build_col_projection(frame, 0..frame.height(), 98);
        let mut horizontal = detect_axis(&rows, threshold, min_row_band);
        let mut vertical = detect_axis(&cols, threshold, min_col_band);

        // Exclude the other axis's bars before inspecting narrow content.
        if let Some((left, right)) = vertical
            && left + right > 0
        {
            let rows = self.build_row_projection(frame, left..frame.width() - right, 98);
            horizontal = detect_axis(&rows, threshold, min_row_band);
        }
        if let Some((top, bottom)) = horizontal
            && top + bottom > 0
        {
            let cols = self.build_col_projection(frame, top..frame.height() - bottom, 98);
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
        let mut projection = Vec::with_capacity(frame.height() as usize);
        for y in 0..frame.height() {
            self.sample_scratch.clear();
            for x in columns.clone().step_by(SAMPLE_STRIDE) {
                self.sample_scratch.push(luma_at(frame, x, y));
            }
            projection.push(percentile(&mut self.sample_scratch, percent));
        }
        projection
    }

    /// Read strided rows sequentially before taking each column's percentile.
    fn build_col_projection(&mut self, frame: &Frame, rows: Range<u32>, percent: usize) -> Vec<u8> {
        let width = frame.width() as usize;
        let n_rows = rows.clone().step_by(SAMPLE_STRIDE).count();
        self.grid_scratch.resize(width * n_rows, 0);
        for (k, y) in rows.step_by(SAMPLE_STRIDE).enumerate() {
            for x in 0..frame.width() {
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

    pub fn process(&mut self, mut input: FrameView, now: Instant) -> FrameView {
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
            width: input.frame.width() - left - right,
            height: input.frame.height() - top - bottom,
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
    // Letterboxing and pillarboxing require a pair, even for thin margins.
    if start == 0 || end == 0 {
        return Some((0, 0));
    }
    // shortcut: require 10% picture extent to reject isolated highlights;
    // revisit this limit if tiny inset videos need support.
    if brightness.len() - start as usize - (end as usize) < brightness.len().div_ceil(10) {
        return None;
    }
    // Strongly unequal margins are often dark scenery; keep only their shared depth.
    // shortcut: allow 2.5% framing offset; larger offsets retain some real black border.
    if start.abs_diff(end) as usize > (brightness.len() / 40).max(2) {
        return Some((start.min(end), start.min(end)));
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
mod tests;
