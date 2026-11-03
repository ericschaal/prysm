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
const DEAD_BAND: u32 = 2;
const SECTORS: usize = 12;
const SAMPLES: usize = 8;

/// Estimates four independent picture boundaries and holds them through ambiguity.
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
    since: Instant,
    last_seen: Instant,
    samples: VecDeque<u32>,
}

fn matches(a: u32, b: u32) -> bool {
    a.abs_diff(b) <= DEAD_BAND && (a == 0) == (b == 0)
}

impl EdgeCrop {
    fn update(&mut self, observed: Option<u32>, now: Instant) {
        while self
            .recent_content
            .front()
            .is_some_and(|(_, time)| now.saturating_duration_since(*time) >= INWARD_CONFIRM)
        {
            self.recent_content.pop_front();
        }
        let Some(observed) = observed else {
            self.candidate = None;
            return;
        };
        self.recent_content.push_back((observed, now));
        if matches(observed, self.current) {
            self.candidate = None;
            return;
        }
        let candidate = self.candidate.get_or_insert_with(|| Candidate {
            anchor: observed,
            since: now,
            last_seen: now,
            samples: VecDeque::new(),
        });
        if !matches(observed, candidate.anchor)
            || now.saturating_duration_since(candidate.last_seen) > EVIDENCE_GAP
        {
            *candidate = Candidate {
                anchor: observed,
                since: now,
                last_seen: now,
                samples: VecDeque::new(),
            };
        }
        candidate.last_seen = now;
        candidate.samples.push_back(observed);
        if candidate.samples.len() > 32 {
            candidate.samples.pop_front();
        }
        let inward = observed > self.current;
        let confirmation = if inward {
            INWARD_CONFIRM
        } else {
            OUTWARD_CONFIRM
        };
        if now.saturating_duration_since(candidate.since) < confirmation {
            return;
        }
        let mut positions: Vec<_> = candidate.samples.iter().copied().collect();
        let middle = positions.len() / 2;
        let proposed = *positions.select_nth_unstable(middle).1;
        if inward
            && self
                .recent_content
                .iter()
                .any(|&(position, _)| position.saturating_add(DEAD_BAND) < proposed)
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
        if frame.width() == 0 || frame.height() == 0 || !informative(frame) {
            return [None; 4];
        }
        let full = Viewport::full_frame(frame.width(), frame.height());
        let initial = std::array::from_fn(|edge| observe_edge(frame, edge, full, 2, 0));
        // Restrict the transverse search before requiring distributed support in an inset picture.
        let provisional = viewport(frame, initial.map(|value| value.unwrap_or(0))).unwrap_or(full);
        std::array::from_fn(|edge| {
            observe_edge(frame, edge, provisional, 4, self.edges[edge].current)
        })
    }

    pub fn process(&mut self, mut input: FrameView, now: Instant) -> FrameView {
        if self
            .last_scan
            .is_none_or(|last| now.saturating_duration_since(last) >= SCAN_INTERVAL)
        {
            self.last_scan = Some(now);
            let observed = self.observe(&input.frame);
            let previous = self.edges.each_ref().map(|edge| edge.current);
            for (edge, observation) in self.edges.iter_mut().zip(observed) {
                edge.update(observation, now);
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

#[allow(
    clippy::cast_possible_truncation,
    reason = "partition coordinates remain within the u32 frame dimensions"
)]
fn observe_edge(
    frame: &Frame,
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
    };
    let cross = if horizontal {
        picture.x..picture.x + picture.width
    } else {
        picture.y..picture.y + picture.height
    };
    let sustained = (frame.width().min(frame.height()) / 18).max(3);
    let verification = if required >= 4 {
        sustained
            .max((picture.width.min(picture.height) / 5).min(frame.width().min(frame.height()) / 9))
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
            *observation = sector_boundary(frame, edge, start..end, extent, current, verification);
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
                .filter(|&value| matches(value, boundary))
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
    result.filter(|&boundary| {
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

fn sector_boundary(
    frame: &Frame,
    edge: usize,
    cross: Range<u32>,
    extent: u32,
    current: u32,
    verification: u32,
) -> Sector {
    let ceiling = black_ceiling(frame);
    let (base, _, _) = line(frame, edge, 0, cross.clone(), ceiling);
    // Weak detail can rule out a crop without being strong enough to locate its boundary.
    let threshold = ceiling.min(base.saturating_add(3));
    let sustained = (frame.width().min(frame.height()) / 18).max(3);
    let uncertainty = (frame.width().min(frame.height()) / 90).max(2);
    let max_border = extent - extent.div_ceil(10);
    let mut weak = ContentRun::default();
    let mut strong = ContentRun::default();
    let mut content = None;
    for depth in 0..extent.min(max_border.saturating_add(verification)) {
        let (median, visible, chromatic) = line(frame, edge, depth, cross.clone(), threshold);
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
        } => 23,
        _ => 8,
    }
}

fn pixel(frame: &Frame, x: u32, y: u32) -> (u8, u8) {
    let chroma = if matches!(frame.format(), PixelFormat::YUYV { .. }) {
        let index = (y as usize * frame.width() as usize + (x as usize & !1)) * 2;
        frame.as_slice()[index + 1]
            .abs_diff(128)
            .max(frame.as_slice()[index + 3].abs_diff(128))
    } else {
        0
    };
    (luma_at(frame, x, y), chroma)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "sample coordinates remain within the u32 frame dimensions"
)]
fn informative(frame: &Frame) -> bool {
    let mut count = 0;
    for y in 0..12_u64 {
        for x in 0..12_u64 {
            let (level, chroma) = pixel(
                frame,
                (u64::from(frame.width()) * (2 * x + 1) / 24) as u32,
                (u64::from(frame.height()) * (2 * y + 1) / 24) as u32,
            );
            count += usize::from(level >= black_ceiling(frame) + 16 || chroma > 16);
            if count >= 4 {
                return true;
            }
        }
    }
    false
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "sample coordinates remain within the u32 frame dimensions"
)]
fn line(
    frame: &Frame,
    edge: usize,
    depth: u32,
    cross: Range<u32>,
    threshold: u8,
) -> (u8, usize, usize) {
    let mut levels = [0; SAMPLES];
    let mut visible = 0;
    let mut chromatic = 0;
    for (sample, level) in levels.iter_mut().enumerate() {
        let position = cross.start
            + ((u64::from(cross.end - cross.start) * (2 * sample + 1) as u64)
                / (2 * SAMPLES) as u64) as u32;
        let (x, y) = match edge {
            0 => (position, depth),
            1 => (frame.width() - 1 - depth, position),
            2 => (position, frame.height() - 1 - depth),
            _ => (depth, position),
        };
        let (brightness, chroma) = pixel(frame, x, y);
        *level = brightness;
        chromatic += usize::from(chroma > 8);
        visible += usize::from(brightness > threshold || chroma > 8);
    }
    let median = *levels.select_nth_unstable(SAMPLES / 2).1;
    (median, visible, chromatic)
}

#[cfg(test)]
mod tests;
