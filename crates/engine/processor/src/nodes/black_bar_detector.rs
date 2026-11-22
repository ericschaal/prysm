use super::spatial_sampler::lattice_step;
use crate::frames::{FrameView, PixelView, Viewport};
use prysm_core::{Edge, Frame, PixelFormat, YuvRange};
use std::collections::VecDeque;
use std::ops::Range;
use std::time::{Duration, Instant};

const SCAN_INTERVAL: Duration = Duration::from_millis(50);
const INWARD_CONFIRM: Duration = Duration::from_millis(1500);
const OUTWARD_CONFIRM: Duration = Duration::from_millis(400);
// shortcut: cropping needs at least 5 fps; adapt the gap if low-rate capture becomes useful.
const EVIDENCE_GAP: Duration = Duration::from_millis(200);
/// Boundaries closer than this are the same boundary.
const DEAD_BAND: Steps = Steps(2);
/// Each edge is searched in this many sectors across it, so that a logo or a
/// caption in one sector cannot set the boundary alone.
const SECTORS: usize = 12;
/// Blocks read across one sector's line; their median is the line's level.
const SAMPLES: usize = 8;
/// Sectors that must agree on a boundary in the first pass, over the whole frame.
const FIRST_PASS_SUPPORT: usize = 2;
/// Sectors that must agree in the second pass, over the provisional picture,
/// spread across at least half of them: bars span the picture, overlays do not.
const SUPPORT: usize = 4;

/// The brightest limited-range luma that is black: black (16) plus noise.
const LIMITED_BLACK_CEILING: u8 = 23;
/// The brightest full-range luma that is black: black (0) plus noise.
const FULL_BLACK_CEILING: u8 = 8;
/// Luma above a bar's own level that is weak detail: enough to rule out a
/// crop, not to place a boundary.
const WEAK_LUMA: u8 = 3;
/// Luma above a bar's own level that places a boundary.
const STRONG_LUMA: u8 = 8;
/// Chroma away from neutral that is colour, which black bars never have.
const COLORED: u8 = 8;
/// Lines without detail that end a run of detail, so grain does not split it.
const RUN_GAP: u32 = 3;

/// A frame is read on a grid of this many blocks each way to tell whether it
/// is bright or colourful enough to show its bars at all.
const INFORMATIVE_GRID: u64 = 12;
/// Blocks of that grid that must be lit or coloured; fewer is a dark scene.
const INFORMATIVE_BLOCKS: usize = 4;
/// Luma above black, or chroma away from neutral, that makes a block lit or
/// coloured.
const INFORMATIVE_LEVEL: u8 = 16;

/// The edges in the order of [`BlackBarDetector`]'s crops and [`viewport`].
const EDGES: [Edge; 4] = [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left];

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
///
/// Together: the edges that confirm a move in the same scan move together,
/// and only if the picture they leave is at least a tenth of the frame each
/// way; otherwise their evidence starts over. The other edges keep theirs.
#[derive(Debug, Default)]
pub struct BlackBarDetector {
    /// In the order of [`EDGES`].
    edges: [EdgeCrop; 4],
    last_scan: Option<Instant>,
}

/// A distance in from an edge, in source pixels; 0 is no crop.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Px(u32);

/// A distance in from an edge, in lattice steps of [`Picture::step`] pixels;
/// 0 is no crop.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Steps(u32);

impl Px {
    fn steps(self, step: u32) -> Steps {
        Steps(self.0 / step)
    }

    fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    fn matches(self, other: Self, tolerance: Self) -> bool {
        same_boundary(self.0, other.0, tolerance.0)
    }
}

impl Steps {
    fn px(self, step: u32) -> Px {
        Px(self.0 * step)
    }

    fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    fn matches(self, other: Self, tolerance: Self) -> bool {
        same_boundary(self.0, other.0, tolerance.0)
    }
}

/// Whether two distances are the same boundary: within `tolerance` of each
/// other, and both a crop or both none.
fn same_boundary(a: u32, b: u32, tolerance: u32) -> bool {
    a.abs_diff(b) <= tolerance && (a == 0) == (b == 0)
}

/// One edge's confirmed crop, and the evidence for a new one.
#[derive(Debug, Default)]
struct EdgeCrop {
    current: Px,
    candidate: Option<Candidate>,
    /// Where scans of the last [`INWARD_CONFIRM`] saw the picture begin.
    recent_content: VecDeque<Content>,
}

/// A boundary other than the current crop, and the scans that saw it.
#[derive(Debug)]
struct Candidate {
    anchor: Px,
    seen: VecDeque<Sighting>,
}

/// One scan that saw the candidate's boundary.
#[derive(Debug, Clone, Copy)]
struct Sighting {
    time: Instant,
    /// The time since the previous scan that counts toward confirmation.
    credit: Duration,
    position: Px,
}

/// One scan's boundary, picture beginning there.
#[derive(Debug, Clone, Copy)]
struct Content {
    position: Px,
    time: Instant,
}

impl EdgeCrop {
    /// Takes one scan's boundary (`None`: the scan could not tell) at `now`,
    /// `elapsed` since the previous scan or zero after a gap, and returns the
    /// crop the evidence with this scan confirms, if any. The crop changes only by
    /// [`Self::commit`]; until then the candidate keeps its evidence.
    fn propose(
        &mut self,
        observed: Option<Px>,
        now: Instant,
        elapsed: Duration,
        tolerance: Px,
    ) -> Option<Px> {
        while self
            .recent_content
            .front()
            .is_some_and(|content| now.saturating_duration_since(content.time) >= INWARD_CONFIRM)
        {
            self.recent_content.pop_front();
        }
        let observed = observed?;
        self.recent_content.push_back(Content {
            position: observed,
            time: now,
        });
        if observed.matches(self.current, tolerance) {
            self.candidate = None;
            return None;
        }
        let (candidate, credit) = match &mut self.candidate {
            Some(candidate) if observed.matches(candidate.anchor, tolerance) => {
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
        candidate.seen.push_back(Sighting {
            time: now,
            credit,
            position: observed,
        });
        let inward = observed > self.current;
        let confirmation = if inward {
            INWARD_CONFIRM
        } else {
            OUTWARD_CONFIRM
        };
        while candidate
            .seen
            .front()
            .is_some_and(|sighting| now.saturating_duration_since(sighting.time) > 2 * confirmation)
        {
            candidate.seen.pop_front();
        }
        let evidence: Duration = candidate.seen.iter().map(|sighting| sighting.credit).sum();
        if evidence < confirmation {
            return None;
        }
        let mut positions: Vec<_> = candidate
            .seen
            .iter()
            .map(|sighting| sighting.position)
            .collect();
        let middle = positions.len() / 2;
        let proposed = *positions.select_nth_unstable(middle).1;
        let picture_outside = self
            .recent_content
            .iter()
            .any(|content| content.position.saturating_add(tolerance) < proposed);
        (!(inward && picture_outside)).then_some(proposed)
    }

    /// Moves the crop to a proposal; its evidence is used up.
    fn commit(&mut self, crop: Px) {
        self.current = crop;
        self.candidate = None;
    }

    /// Drops a proposal the other edges' crops do not allow: its evidence
    /// starts over.
    fn reject(&mut self) {
        self.candidate = None;
    }
}

impl BlackBarDetector {
    fn observe(&self, frame: &Frame) -> [Option<Px>; 4] {
        let Some(picture) = Picture::new(frame) else {
            return [None; 4];
        };
        if !informative(&picture) {
            return [None; 4];
        }
        let full = Viewport::full_frame(frame.width(), frame.height());
        let initial =
            EDGES.map(|edge| observe_edge(&picture, edge, full, FIRST_PASS_SUPPORT, Px(0)));
        // Restrict the transverse search before requiring distributed support in an inset picture.
        let provisional = viewport(frame, initial.map(Option::unwrap_or_default)).unwrap_or(full);
        std::array::from_fn(|index| {
            let current = self.edges[index].current;
            observe_edge(&picture, EDGES[index], provisional, SUPPORT, current)
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
            let tolerance = DEAD_BAND.px(lattice_step(input.frame.width(), input.frame.height()));
            let mut proposals = [None; 4];
            for ((edge, observation), proposal) in
                self.edges.iter_mut().zip(observed).zip(&mut proposals)
            {
                *proposal = edge.propose(observation, now, elapsed, tolerance);
            }
            self.commit(&input.frame, proposals);
        }
        input.viewport = viewport(&input.frame, self.crops())
            .unwrap_or_else(|| Viewport::full_frame(input.frame.width(), input.frame.height()));
        input
    }

    /// The confirmed crops, in the order of [`EDGES`].
    fn crops(&self) -> [Px; 4] {
        self.edges.each_ref().map(|edge| edge.current)
    }

    /// Commits the edges' proposals together if the picture they leave is
    /// large enough ([`viewport`]), and rejects them all otherwise. Edges
    /// without a proposal keep their evidence either way.
    fn commit(&mut self, frame: &Frame, proposals: [Option<Px>; 4]) {
        let crops =
            std::array::from_fn(|index| proposals[index].unwrap_or(self.edges[index].current));
        let accepted = viewport(frame, crops).is_some();
        for (edge, proposal) in self.edges.iter_mut().zip(proposals) {
            match (proposal, accepted) {
                (Some(crop), true) => edge.commit(crop),
                (Some(_), false) => edge.reject(),
                (None, _) => {}
            }
        }
    }
}

/// The picture inside crops `[top, right, bottom, left]`; `None` when it is
/// narrower or lower than a tenth of the frame.
fn viewport(frame: &Frame, [top, right, bottom, left]: [Px; 4]) -> Option<Viewport> {
    let width = frame.width().checked_sub(left.0.checked_add(right.0)?)?;
    let height = frame.height().checked_sub(top.0.checked_add(bottom.0)?)?;
    // shortcut: pictures narrower than 10% of the frame remain uncropped; revisit for tiny insets.
    (width >= frame.width().div_ceil(10) && height >= frame.height().div_ceil(10)).then_some(
        Viewport {
            x: left.0,
            y: top.0,
            width,
            height,
        },
    )
}

/// One frame as the detector reads it.
struct Picture<'a> {
    pixels: PixelView<'a>,
    width: u32,
    height: u32,
    /// The lattice step: the detector reads the frame as if about 640x360.
    step: u32,
    /// The brightest luma that is black.
    ceiling: u8,
}

impl<'a> Picture<'a> {
    /// `None` for an empty frame.
    fn new(frame: &'a Frame) -> Option<Self> {
        let (width, height) = (frame.width(), frame.height());
        if width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            pixels: PixelView::new(frame),
            width,
            height,
            step: lattice_step(width, height),
            ceiling: black_ceiling(frame),
        })
    }
}

/// Run lengths of one edge's search, in lattice steps, scaled to the frame's
/// short side so that every capture size decides alike.
#[derive(Debug, Clone, Copy)]
struct Runs {
    /// Lines of detail that make picture: 1/18 of the short side (20 lines
    /// at 360), at least 3, so a thin bright line does not.
    sustained: u32,
    /// Lines of strong detail that place a boundary: `sustained`, or in the
    /// second pass a fifth of the provisional picture's short side, capped at 1/9 of the
    /// frame's and at least `sustained`: a larger picture must show more.
    verification: u32,
    /// How far inside a boundary weak detail may begin and still belong to
    /// it: 1/90 of the short side, at least 2.
    uncertainty: u32,
}

impl Runs {
    fn new(picture: &Picture, area: Viewport, required: usize) -> Self {
        let step = picture.step;
        let short = picture.width.min(picture.height) / step;
        let sustained = (short / 18).max(3);
        let verification = if required >= SUPPORT {
            sustained.max((area.width.min(area.height) / step / 5).min(short / 9))
        } else {
            sustained
        };
        Self {
            sustained,
            verification,
            uncertainty: (short / 90).max(2),
        }
    }
}

/// One edge's boundary, searched within `area`; `current` is that edge's
/// confirmed crop. The search itself counts depths in lattice steps.
#[allow(
    clippy::cast_possible_truncation,
    reason = "partition coordinates remain within the u32 frame dimensions"
)]
fn observe_edge(
    picture: &Picture,
    edge: Edge,
    area: Viewport,
    required: usize,
    current: Px,
) -> Option<Px> {
    let step = picture.step;
    let horizontal = matches!(edge, Edge::Top | Edge::Bottom);
    let extent = if horizontal {
        picture.height
    } else {
        picture.width
    } / step;
    let cross = if horizontal {
        area.x..area.x + area.width
    } else {
        area.y..area.y + area.height
    };
    let current = current.steps(step);
    let runs = Runs::new(picture, area, required);
    let mut observations = [Sector::default(); SECTORS];
    for (sector, observation) in observations.iter_mut().enumerate() {
        let start = cross.start
            + (u64::from(cross.end - cross.start) * sector as u64 / SECTORS as u64) as u32;
        let end = cross.start
            + (u64::from(cross.end - cross.start) * (sector + 1) as u64 / SECTORS as u64) as u32;
        if start < end {
            *observation = sector_boundary(picture, edge, start..end, extent, current, runs);
        }
    }
    let mut result = None;
    for boundary in observations.iter().filter_map(|sector| sector.boundary) {
        let mut supported = [Steps(0); SECTORS];
        let mut count = 0;
        let mut first = SECTORS;
        let mut last = 0;
        for (sector, observation) in observations.iter().enumerate() {
            if let Some(value) = observation
                .boundary
                .filter(|&value| value.matches(boundary, DEAD_BAND))
            {
                supported[count] = value;
                count += 1;
                first = first.min(sector);
                last = last.max(sector);
            }
        }
        if count >= required && (required < SUPPORT || last - first >= SECTORS / 2) {
            let boundary = *supported[..count].select_nth_unstable(count / 2).1;
            result = Some(result.map_or(boundary, |old: Steps| old.min(boundary)));
        }
    }
    result
        .filter(|&boundary| {
            // Sparse detail inside the known picture vetoes cropping; an overlay in the old bar does not.
            current == Steps(0)
                || !observations
                    .iter()
                    .filter_map(|sector| sector.content)
                    .any(|position| {
                        position.saturating_add(DEAD_BAND) >= current
                            && position.saturating_add(DEAD_BAND) < boundary
                    })
        })
        .map(|boundary| boundary.px(step))
}

/// What one sector of an edge shows: where the picture's boundary is, and
/// where any detail at all begins.
#[derive(Debug, Default, Clone, Copy)]
struct Sector {
    boundary: Option<Steps>,
    content: Option<Steps>,
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
            // Long enough, with detail on at least three lines in four.
            if depth - beginning + 1 >= sustained
                && u64::from(self.count) * 4 >= u64::from(depth - beginning + 1) * 3
            {
                return Some(beginning);
            }
        } else if self.start.is_some() {
            self.gap += 1;
            if self.gap >= RUN_GAP {
                *self = Self::default();
            }
        }
        None
    }
}

/// Depths and the result in lattice steps.
fn sector_boundary(
    picture: &Picture,
    edge: Edge,
    cross: Range<u32>,
    extent: u32,
    Steps(current): Steps,
    runs: Runs,
) -> Sector {
    let Runs {
        sustained,
        verification,
        uncertainty,
    } = runs;
    let ceiling = picture.ceiling;
    let positions = positions(&cross);
    let (base, _, _) = line(picture, edge, 0, &positions, ceiling);
    // Weak detail can rule out a crop without being strong enough to locate its boundary.
    let threshold = ceiling.min(base.saturating_add(WEAK_LUMA));
    let max_border = extent - extent.div_ceil(10);
    let mut weak = ContentRun::default();
    let mut strong = ContentRun::default();
    let mut content = None;
    for depth in 0..extent.min(max_border.saturating_add(verification)) {
        let (median, visible, chromatic) = line(picture, edge, depth, &positions, threshold);
        if content.is_none() {
            content = weak.observe(visible >= SAMPLES / 4, depth, sustained);
        }
        let strong_pixel =
            median > ceiling.min(base.saturating_add(STRONG_LUMA)) || chromatic >= SAMPLES / 2;
        // A new outer boundary must connect through the old matte, rather than stop at a caption.
        if let Some(boundary) = strong.observe(strong_pixel, depth, verification)
            && depth.saturating_add(1) >= current.saturating_add(sustained)
        {
            return Sector {
                boundary: (boundary <= max_border
                    && content
                        .is_none_or(|position| position.saturating_add(uncertainty) >= boundary))
                .then_some(Steps(boundary)),
                content: content.map(Steps),
            };
        }
    }
    Sector {
        boundary: None,
        content: content.map(Steps),
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
        } => LIMITED_BLACK_CEILING,
        _ => FULL_BLACK_CEILING,
    }
}

/// Mean luma and chroma of the `step` x `step` block at (`x`, `y`), read along
/// its diagonal: `step` pixels that cover every row and column of the block once.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the mean of u8 values fits in u8"
)]
fn block(picture: &Picture, x: u32, y: u32) -> (u8, u8) {
    let step = picture.step;
    if step == 1 {
        return picture.pixels.luma_and_chroma(x, y);
    }
    let (mut luma, mut chroma) = (0, 0);
    for i in 0..step {
        let (level, color) = picture.pixels.luma_and_chroma(
            (x + i).min(picture.width - 1),
            (y + i).min(picture.height - 1),
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
fn informative(picture: &Picture) -> bool {
    let mut count = 0;
    for y in 0..INFORMATIVE_GRID {
        for x in 0..INFORMATIVE_GRID {
            let (level, chroma) = block(
                picture,
                (u64::from(picture.width) * (2 * x + 1) / (2 * INFORMATIVE_GRID)) as u32,
                (u64::from(picture.height) * (2 * y + 1) / (2 * INFORMATIVE_GRID)) as u32,
            );
            count += usize::from(
                level >= picture.ceiling + INFORMATIVE_LEVEL || chroma > INFORMATIVE_LEVEL,
            );
            if count >= INFORMATIVE_BLOCKS {
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
    picture: &Picture,
    edge: Edge,
    depth: u32,
    positions: &[u32; SAMPLES],
    threshold: u8,
) -> (u8, usize, usize) {
    let mut levels = [0; SAMPLES];
    let mut visible = 0;
    let mut chromatic = 0;
    let step = picture.step;
    let inset = depth * step;
    for (level, &position) in levels.iter_mut().zip(positions) {
        let (x, y) = match edge {
            Edge::Top => (position, inset),
            Edge::Right => (picture.width.saturating_sub(inset + step), position),
            Edge::Bottom => (position, picture.height.saturating_sub(inset + step)),
            Edge::Left => (inset, position),
        };
        let (brightness, chroma) = block(picture, x, y);
        *level = brightness;
        chromatic += usize::from(chroma > COLORED);
        visible += usize::from(brightness > threshold || chroma > COLORED);
    }
    let median = *levels.select_nth_unstable(SAMPLES / 2).1;
    (median, visible, chromatic)
}

#[cfg(test)]
mod tests;
