use super::spatial_sampler::lattice_step;
use crate::frames::{FrameView, Viewport, luma_at};
use prysm_core::{Frame, PixelFormat, YuvRange};
use std::collections::VecDeque;
use std::ops::Range;
use std::time::{Duration, Instant};

const SCAN_INTERVAL: Duration = Duration::from_millis(50);
const INWARD_CONFIRM: Duration = Duration::from_millis(1500);
const OUTWARD_CONFIRM: Duration = Duration::from_millis(400);
// shortcut: cropping needs at least 5 fps; adapt the gap if low-rate capture becomes useful.
const EVIDENCE_GAP: Duration = Duration::from_millis(200);
/// Boundaries closer than this many lattice steps are the same boundary.
const DEAD_BAND: u32 = 2;
const SECTORS: usize = 12;
const SAMPLES: usize = 8;

/// Estimates four independent picture boundaries and holds them through ambiguity.
///
/// Size: the detector reads the picture as if it were about 640x360. Frames above
/// that are read on the sampler's lattice of every nth row and column (n = 2 at
/// 1280x720, 3 at 1920x1080), and each lattice sample averages n pixels of its
/// n x n block, much like a downscaler. Depths, run lengths and tolerances are
/// counted in lattice steps, so the same picture gives the same decision at any
/// capture size, at about the same cost.
///
/// Time: every 50 ms each edge reports a boundary, or nothing when the picture is
/// too dark or ambiguous to tell. Each sighting of a new boundary counts the time
/// since the previous scan (none for the first sighting, or after a gap over
/// 200 ms). An edge moves to the new boundary once its sightings within the last
/// 3 s add up to 1.5 s when cropping more (inward), or within the last 0.8 s to
/// 0.4 s when cropping less: at least half of the scans must see it. Ambiguous scans neither count
/// nor reset that evidence, so a dark letterboxed scene crops once half of its
/// scans see the bars, while seeing a different boundary starts over. An inward
/// move also waits until no scan of the last 1.5 s saw picture outside the new
/// boundary. Bars in a lit picture are seen on every scan and crop after 1.5 s
/// (0.4 s when they shrink); a dark scene without bars stays uncropped.
#[derive(Debug, Default)]
pub struct BlackBarDetector {
    // Top, right, bottom, left, in source pixels.
    edges: [EdgeCrop; 4],
    last_scan: Option<Instant>,
}

#[derive(Debug, Default)]
struct EdgeCrop {
    current: u32,
    candidate: Option<Candidate>,
    recent_content: VecDeque<(u32, Instant)>,
}

#[derive(Debug)]
struct Candidate {
    anchor: u32,
    /// Scans that saw this boundary: time, credited duration, position.
    seen: VecDeque<(Instant, Duration, u32)>,
}

fn matches(a: u32, b: u32, tolerance: u32) -> bool {
    a.abs_diff(b) <= tolerance && (a == 0) == (b == 0)
}

impl EdgeCrop {
    /// `observed` and `tolerance` in source pixels; `elapsed` since the previous
    /// scan, or zero after a gap.
    fn update(&mut self, observed: Option<u32>, now: Instant, elapsed: Duration, tolerance: u32) {
        while self
            .recent_content
            .front()
            .is_some_and(|(_, time)| now.saturating_duration_since(*time) >= INWARD_CONFIRM)
        {
            self.recent_content.pop_front();
        }
        let Some(observed) = observed else {
            return;
        };
        self.recent_content.push_back((observed, now));
        if matches(observed, self.current, tolerance) {
            self.candidate = None;
            return;
        }
        let (candidate, credit) = match &mut self.candidate {
            Some(candidate) if matches(observed, candidate.anchor, tolerance) => {
                (candidate, elapsed)
            }
            _ => (
                self.candidate.insert(Candidate {
                    anchor: observed,
                    seen: VecDeque::new(),
                }),
                Duration::ZERO,
            ),
        };
        candidate.seen.push_back((now, credit, observed));
        let inward = observed > self.current;
        let confirmation = if inward {
            INWARD_CONFIRM
        } else {
            OUTWARD_CONFIRM
        };
        while candidate
            .seen
            .front()
            .is_some_and(|(time, _, _)| now.saturating_duration_since(*time) > 2 * confirmation)
        {
            candidate.seen.pop_front();
        }
        if candidate
            .seen
            .iter()
            .map(|(_, credit, _)| *credit)
            .sum::<Duration>()
            < confirmation
        {
            return;
        }
        let mut positions: Vec<_> = candidate
            .seen
            .iter()
            .map(|(_, _, position)| *position)
            .collect();
        let middle = positions.len() / 2;
        let proposed = *positions.select_nth_unstable(middle).1;
        if inward
            && self
                .recent_content
                .iter()
                .any(|&(position, _)| position.saturating_add(tolerance) < proposed)
        {
            return;
        }
        self.current = proposed;
        self.candidate = None;
    }
}

impl BlackBarDetector {
    pub fn new() -> Self {
        Self::default()
    }

    fn observe(&self, frame: &Frame) -> [Option<u32>; 4] {
        let step = lattice_step(frame.width(), frame.height());
        if frame.width() == 0 || frame.height() == 0 || !informative(frame, step) {
            return [None; 4];
        }
        let full = Viewport::full_frame(frame.width(), frame.height());
        let initial = std::array::from_fn(|edge| observe_edge(frame, step, edge, full, 2, 0));
        // Restrict the transverse search before requiring distributed support in an inset picture.
        let provisional = viewport(frame, initial.map(|value| value.unwrap_or(0))).unwrap_or(full);
        std::array::from_fn(|edge| {
            observe_edge(frame, step, edge, provisional, 4, self.edges[edge].current)
        })
    }

    pub fn process(&mut self, mut input: FrameView, now: Instant) -> FrameView {
        if self
            .last_scan
            .is_none_or(|last| now.saturating_duration_since(last) >= SCAN_INTERVAL)
        {
            let elapsed = self
                .last_scan
                .map(|last| now.saturating_duration_since(last))
                .filter(|&gap| gap <= EVIDENCE_GAP)
                .unwrap_or_default();
            self.last_scan = Some(now);
            let observed = self.observe(&input.frame);
            let tolerance = DEAD_BAND * lattice_step(input.frame.width(), input.frame.height());
            let previous = self.edges.each_ref().map(|edge| edge.current);
            for (edge, observation) in self.edges.iter_mut().zip(observed) {
                edge.update(observation, now, elapsed, tolerance);
            }
            if viewport(&input.frame, self.edges.each_ref().map(|edge| edge.current)).is_none() {
                for (edge, current) in self.edges.iter_mut().zip(previous) {
                    edge.current = current;
                    edge.candidate = None;
                }
            }
        }
        input.viewport = viewport(&input.frame, self.edges.each_ref().map(|edge| edge.current))
            .unwrap_or_else(|| Viewport::full_frame(input.frame.width(), input.frame.height()));
        input
    }
}

fn viewport(frame: &Frame, [top, right, bottom, left]: [u32; 4]) -> Option<Viewport> {
    let width = frame.width().checked_sub(left.checked_add(right)?)?;
    let height = frame.height().checked_sub(top.checked_add(bottom)?)?;
    // shortcut: pictures narrower than 10% of the frame remain uncropped; revisit for tiny insets.
    (width >= frame.width().div_ceil(10) && height >= frame.height().div_ceil(10)).then_some(
        Viewport {
            x: left,
            y: top,
            width,
            height,
        },
    )
}

/// One edge's boundary in source pixels; `current` is that edge's confirmed crop.
/// The search itself counts depths in lattice steps of `step` pixels.
#[allow(
    clippy::cast_possible_truncation,
    reason = "partition coordinates remain within the u32 frame dimensions"
)]
fn observe_edge(
    frame: &Frame,
    step: u32,
    edge: usize,
    picture: Viewport,
    required: usize,
    current: u32,
) -> Option<u32> {
    let horizontal = edge == 0 || edge == 2;
    let extent = if horizontal {
        frame.height()
    } else {
        frame.width()
    } / step;
    let cross = if horizontal {
        picture.x..picture.x + picture.width
    } else {
        picture.y..picture.y + picture.height
    };
    let current = current / step;
    let short = frame.width().min(frame.height()) / step;
    let sustained = (short / 18).max(3);
    let verification = if required >= 4 {
        sustained.max((picture.width.min(picture.height) / step / 5).min(short / 9))
    } else {
        sustained
    };
    let mut observations = [Sector::default(); SECTORS];
    for (sector, observation) in observations.iter_mut().enumerate() {
        let start = cross.start
            + (u64::from(cross.end - cross.start) * sector as u64 / SECTORS as u64) as u32;
        let end = cross.start
            + (u64::from(cross.end - cross.start) * (sector + 1) as u64 / SECTORS as u64) as u32;
        if start < end {
            *observation =
                sector_boundary(frame, step, edge, start..end, extent, current, verification);
        }
    }
    let mut result = None;
    for boundary in observations.iter().filter_map(|sector| sector.boundary) {
        let mut supported = [0; SECTORS];
        let mut count = 0;
        let mut first = SECTORS;
        let mut last = 0;
        for (sector, observation) in observations.iter().enumerate() {
            if let Some(value) = observation
                .boundary
                .filter(|&value| matches(value, boundary, DEAD_BAND))
            {
                supported[count] = value;
                count += 1;
                first = first.min(sector);
                last = last.max(sector);
            }
        }
        if count >= required && (required < 4 || last - first >= SECTORS / 2) {
            let boundary = *supported[..count].select_nth_unstable(count / 2).1;
            result = Some(result.map_or(boundary, |old: u32| old.min(boundary)));
        }
    }
    result
        .filter(|&boundary| {
            // Sparse detail inside the known picture vetoes cropping; an overlay in the old bar does not.
            current == 0
                || !observations
                    .iter()
                    .filter_map(|sector| sector.content)
                    .any(|position| {
                        position.saturating_add(DEAD_BAND) >= current
                            && position.saturating_add(DEAD_BAND) < boundary
                    })
        })
        .map(|boundary| boundary * step)
}

#[derive(Debug, Default, Clone, Copy)]
struct Sector {
    boundary: Option<u32>,
    content: Option<u32>,
}

#[derive(Default)]
struct ContentRun {
    start: Option<u32>,
    count: u32,
    gap: u32,
}

impl ContentRun {
    fn observe(&mut self, visible: bool, depth: u32, sustained: u32) -> Option<u32> {
        if visible {
            let beginning = *self.start.get_or_insert(depth);
            self.count += 1;
            self.gap = 0;
            if depth - beginning + 1 >= sustained
                && u64::from(self.count) * 4 >= u64::from(depth - beginning + 1) * 3
            {
                return Some(beginning);
            }
        } else if self.start.is_some() {
            self.gap += 1;
            if self.gap >= 3 {
                *self = Self::default();
            }
        }
        None
    }
}

/// Depths and the result in lattice steps.
fn sector_boundary(
    frame: &Frame,
    step: u32,
    edge: usize,
    cross: Range<u32>,
    extent: u32,
    current: u32,
    verification: u32,
) -> Sector {
    let ceiling = black_ceiling(frame);
    let positions = positions(&cross);
    let (base, _, _) = line(frame, step, edge, 0, &positions, ceiling);
    // Weak detail can rule out a crop without being strong enough to locate its boundary.
    let threshold = ceiling.min(base.saturating_add(3));
    let short = frame.width().min(frame.height()) / step;
    let sustained = (short / 18).max(3);
    let uncertainty = (short / 90).max(2);
    let max_border = extent - extent.div_ceil(10);
    let mut weak = ContentRun::default();
    let mut strong = ContentRun::default();
    let mut content = None;
    for depth in 0..extent.min(max_border.saturating_add(verification)) {
        let (median, visible, chromatic) = line(frame, step, edge, depth, &positions, threshold);
        if content.is_none() {
            content = weak.observe(visible >= SAMPLES / 4, depth, sustained);
        }
        let strong_pixel = median > ceiling.min(base.saturating_add(8)) || chromatic >= SAMPLES / 2;
        // A new outer boundary must connect through the old matte, rather than stop at a caption.
        if let Some(boundary) = strong.observe(strong_pixel, depth, verification)
            && depth.saturating_add(1) >= current.saturating_add(sustained)
        {
            return Sector {
                boundary: (boundary <= max_border
                    && content
                        .is_none_or(|position| position.saturating_add(uncertainty) >= boundary))
                .then_some(boundary),
                content,
            };
        }
    }
    Sector {
        boundary: None,
        content,
    }
}

fn black_ceiling(frame: &Frame) -> u8 {
    match frame.format() {
        PixelFormat::YUYV {
            range: YuvRange::Limited,
            ..
        }
        | PixelFormat::NV12 {
            range: YuvRange::Limited,
            ..
        } => 23,
        _ => 8,
    }
}

fn pixel(frame: &Frame, x: u32, y: u32) -> (u8, u8) {
    let chroma = match frame.format() {
        PixelFormat::YUYV { .. } => {
            let index = (y as usize * frame.width() as usize + (x as usize & !1)) * 2;
            frame.as_slice()[index + 1]
                .abs_diff(128)
                .max(frame.as_slice()[index + 3].abs_diff(128))
        }
        PixelFormat::NV12 { .. } => frame.nv12_planes().map_or(0, |(_, uv)| {
            let index = (y as usize / 2) * frame.width() as usize + (x as usize & !1);
            uv[index].abs_diff(128).max(uv[index + 1].abs_diff(128))
        }),
        _ => 0,
    };
    (luma_at(frame, x, y), chroma)
}

/// Mean luma and chroma of the `step` x `step` block at (`x`, `y`), read along
/// its diagonal: `step` pixels that cover every row and column of the block once.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the mean of u8 values fits in u8"
)]
fn block(frame: &Frame, x: u32, y: u32, step: u32) -> (u8, u8) {
    if step == 1 {
        return pixel(frame, x, y);
    }
    let (mut luma, mut chroma) = (0, 0);
    for i in 0..step {
        let (level, color) = pixel(
            frame,
            (x + i).min(frame.width() - 1),
            (y + i).min(frame.height() - 1),
        );
        luma += u32::from(level);
        chroma += u32::from(color);
    }
    ((luma / step) as u8, (chroma / step) as u8)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "sample coordinates remain within the u32 frame dimensions"
)]
fn informative(frame: &Frame, step: u32) -> bool {
    let mut count = 0;
    for y in 0..12_u64 {
        for x in 0..12_u64 {
            let (level, chroma) = block(
                frame,
                (u64::from(frame.width()) * (2 * x + 1) / 24) as u32,
                (u64::from(frame.height()) * (2 * y + 1) / 24) as u32,
                step,
            );
            count += usize::from(level >= black_ceiling(frame) + 16 || chroma > 16);
            if count >= 4 {
                return true;
            }
        }
    }
    false
}

/// The [`SAMPLES`] evenly spread positions across `cross` that [`line`] reads.
#[allow(
    clippy::cast_possible_truncation,
    reason = "sample coordinates remain within the u32 frame dimensions"
)]
fn positions(cross: &Range<u32>) -> [u32; SAMPLES] {
    std::array::from_fn(|sample| {
        cross.start
            + ((u64::from(cross.end - cross.start) * (2 * sample + 1) as u64)
                / (2 * SAMPLES) as u64) as u32
    })
}

/// Median luma, and how many samples are visible or colored, of the lattice line
/// `depth` steps in from `edge`.
fn line(
    frame: &Frame,
    step: u32,
    edge: usize,
    depth: u32,
    positions: &[u32; SAMPLES],
    threshold: u8,
) -> (u8, usize, usize) {
    let mut levels = [0; SAMPLES];
    let mut visible = 0;
    let mut chromatic = 0;
    let inset = depth * step;
    for (level, &position) in levels.iter_mut().zip(positions) {
        let (x, y) = match edge {
            0 => (position, inset),
            1 => (frame.width().saturating_sub(inset + step), position),
            2 => (position, frame.height().saturating_sub(inset + step)),
            _ => (inset, position),
        };
        let (brightness, chroma) = block(frame, x, y, step);
        *level = brightness;
        chromatic += usize::from(chroma > 8);
        visible += usize::from(brightness > threshold || chroma > 8);
    }
    let median = *levels.select_nth_unstable(SAMPLES / 2).1;
    (median, visible, chromatic)
}

#[cfg(test)]
mod tests;
