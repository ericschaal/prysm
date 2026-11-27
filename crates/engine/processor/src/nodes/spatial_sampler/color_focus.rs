//! Colour focus: each sample keeps the hue and saturation of the picture next
//! to it and takes from the rest of the picture only what agrees with it.
//!
//! A plain weighted mean of two different colours is duller than either. For
//! each sample, with strength `s` (0 to 1):
//!
//! 1. The **anchor** is the colour next to the sample: the mean over the
//!    kernel's local lobe alone, with grey cells counted less, as Oklab `a, b`.
//! 2. Each cell's weight is its geometric weight times
//!    `1 - s + s · agreement · grey_factor`, where
//!    `agreement = 1 / (1 + |ab - ab₀|² / σ²)` compares hue and saturation
//!    only (a brighter patch of the same colour agrees fully) and
//!    `grey_factor = 1 + GREY_DISCOUNT · chroma / GREY_CHROMA`.
//! 3. The selective mean of the cells is scaled to the plain mean's Rec. 709
//!    luminance, never past full on any channel.
//!
//! Evaluating step 2 for every sample and cell each frame is too slow for the
//! TV, so it is evaluated in two parts. In a sample's *near field*, where the
//! detail next to the light is (two kernel radii at the default edge depth),
//! cells count one by one, or in pairs at deep edges. Every other cell counts
//! through the coarse block that holds it, with the block's mean colour; about
//! [`POOLED_BLOCKS`] cover the picture. The near groups, and each sample's
//! geometric weight of every block without its near cells, depend on the
//! geometry only and are built once for it. The sizes are the
//! selective-sampler study's: within a just-noticeable step of the formula on
//! every cell, at about the same cost at any edge depth.

use super::{Geometry, cell_area, distance_squared, local_lobe};
use prysm_core::{LinearColor, Oklab};

/// Width σ of hue-and-saturation agreement, in Oklab `a, b`: a cell this far
/// from the anchor counts half. Four just-noticeable steps, the width the
/// colourfulness study modelled.
const AGREEMENT_WIDTH: f32 = 0.08;

/// How much more a cell of [`GREY_CHROMA`] counts than a grey one: grey says
/// little about which colour a light should show. The study's value.
const GREY_DISCOUNT: f32 = 1.0;

/// The Oklab chroma of a clearly coloured surface, the unit of the grey discount.
const GREY_CHROMA: f32 = 0.1;

/// The near field reaches this share of the picture height (two kernel radii
/// at the default edge depth of 10 %), so a deeper edge costs no more...
const NEAR_HEIGHT: f32 = 0.2;

/// ...but no further than two radii, beyond which the kernel's tail falls as
/// the inverse square of the distance and blocks are small next to it...
const NEAR_RADII_MAX: f32 = 2.0;

/// ...and at least one, the kernel's core: a block there holding two colours
/// would misjudge how well they agree with the anchor by more than a step.
const NEAR_RADII_MIN: f32 = 1.0;

/// With a kernel narrower than a block, the blocks next to the sample would
/// carry most of its weight: the near field holds them cell by cell.
const NEAR_MIN_BLOCKS: f32 = 1.5;

/// Near-field groups are at most this many local-lobe widths (a quarter
/// radius) wide, so they never blur the colour next to the light...
const GROUP_LOBES: f32 = 0.5;

/// ...and at most this many cells wide, so they are finer than most picture
/// detail: single cells up to an edge depth of about 35 % at 640x360, pairs
/// beyond.
const GROUP_CELLS: usize = 2;

/// About how many coarse blocks cover the whole picture: 112 at 640x360, each
/// 6x6 cells (47 pixels) wide. Half the cost of 400, within a step of the
/// full evaluation at every edge depth (selective-sampler study).
const POOLED_BLOCKS: usize = 100;

/// Coarse blocks are at least this many near groups wide, so a deeper edge
/// pools as coarsely relative to its groups as the default does.
const BLOCK_GROUPS: usize = 3;

/// Ignore selective colours below this Rec. 709 luminance when scaling hue.
const DARK: f32 = 1e-6;

/// How finely the two-part evaluation looks at the picture; see the
/// constants of the same names.
#[derive(Debug, Clone, Copy)]
struct Sizes {
    near_height: f32,
    near_radii_max: f32,
    group_cells: usize,
    pooled_blocks: usize,
}

impl Sizes {
    const SHIPPED: Self = Self {
        near_height: NEAR_HEIGHT,
        near_radii_max: NEAR_RADII_MAX,
        group_cells: GROUP_CELLS,
        pooled_blocks: POOLED_BLOCKS,
    };
}

/// The selective pass's per-geometry fields and per-frame colours. All
/// storage is kept across frames and geometries.
#[derive(Debug, Default)]
pub(super) struct ColorFocus {
    /// Whether `fields`, `groups` and `blocks` match the sampler's geometry.
    ready: bool,
    fields: Fields,
    /// The near field's resolution.
    groups: Pooling,
    /// The far field's resolution.
    blocks: Pooling,
}

/// The samples in strip order: near groups, and far weights by block.
#[derive(Debug, Default, PartialEq)]
struct Fields {
    /// Sample `i`'s near groups are `near[starts[i]..starts[i + 1]]`.
    starts: Vec<usize>,
    near: Vec<NearGroup>,
    /// One weight per block per sample: the sample's geometric weight of the
    /// block's cells outside its near field, so no cell counts twice.
    far: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NearGroup {
    group: usize,
    /// The group's normalized geometric weight, as in the plain mean.
    weight: f32,
    /// The group's local-lobe weight, for the anchor.
    local: f32,
}

/// Cells pooled into square groups, row by row.
#[derive(Debug, Default)]
struct Pooling {
    /// Each cell's group, and its share of the group's area.
    of_cell: Vec<(usize, f32)>,
    /// Each group's bounding rectangle, `[x0, y0, x1, y1]`.
    rects: Vec<[u32; 4]>,
    /// Whether any sample reads the group; the others need no tint.
    in_use: Vec<bool>,
    // This frame's mean colour of each group.
    colors: Vec<LinearColor>,
    tints: Vec<Tint>,
}

impl Pooling {
    /// Groups of `side` x `side` cells of a grid `columns` wide; the last
    /// row and column may be narrower.
    fn build(&mut self, cells: &[[u32; 4]], columns: usize, side: usize) {
        let group_columns = columns.div_ceil(side);
        let count = group_columns * cells.len().div_ceil(columns).div_ceil(side);
        self.rects.clear();
        self.rects.resize(count, [u32::MAX, u32::MAX, 0, 0]);
        self.in_use.clear();
        self.in_use.resize(count, true);
        let mut areas = vec![0.0_f32; count];
        self.of_cell.clear();
        for (index, &[x0, y0, x1, y1]) in cells.iter().enumerate() {
            let group = index / columns / side * group_columns + index % columns / side;
            debug_assert!(
                group < count,
                "cell {index} lies in group {group} of {count}"
            );
            let rect = &mut self.rects[group];
            *rect = [
                rect[0].min(x0),
                rect[1].min(y0),
                rect[2].max(x1),
                rect[3].max(y1),
            ];
            let area = cell_area([x0, y0, x1, y1]);
            areas[group] += area;
            self.of_cell.push((group, area));
        }
        // Grid dimensions do not exceed the picture: every member cell has positive area.
        for (group, share) in &mut self.of_cell {
            *share /= areas[*group];
        }
    }

    fn len(&self) -> usize {
        self.rects.len()
    }

    /// This frame's mean colour and tint of each group.
    fn pool(&mut self, cell_colors: &[LinearColor]) {
        self.colors.clear();
        self.colors.resize(self.len(), LinearColor::black());
        for (&color, &(group, share)) in cell_colors.iter().zip(&self.of_cell) {
            self.colors[group] += color * share;
        }
        self.tints.clear();
        let groups = self.colors.iter().zip(&self.in_use);
        self.tints.extend(groups.map(|(&color, &in_use)| {
            if in_use {
                Tint::of(color)
            } else {
                Tint::UNUSED
            }
        }));
    }

    /// Group `index`'s colour and tint.
    fn get(&self, index: usize) -> (LinearColor, Tint) {
        (self.colors[index], self.tints[index])
    }
}

/// Hue and saturation: a colour's Oklab `a, b`.
#[derive(Debug, Clone, Copy)]
struct Chromaticity {
    a: f32,
    b: f32,
}

impl Chromaticity {
    fn of(color: LinearColor) -> Self {
        let Oklab { a, b, .. } = Oklab::from_linear(color);
        Self { a, b }
    }

    fn chroma(self) -> f32 {
        self.a.hypot(self.b)
    }
}

/// A colour's chromaticity and how much it counts against grey.
#[derive(Debug, Clone, Copy)]
struct Tint {
    chromaticity: Chromaticity,
    grey_factor: f32,
}

impl Tint {
    /// Stands in for a colour no sample reads.
    const UNUSED: Self = Self {
        chromaticity: Chromaticity { a: 0.0, b: 0.0 },
        grey_factor: 1.0,
    };

    fn of(color: LinearColor) -> Self {
        let chromaticity = Chromaticity::of(color);
        Self {
            chromaticity,
            grey_factor: 1.0 + GREY_DISCOUNT * chromaticity.chroma() / GREY_CHROMA,
        }
    }

    /// 1 at the anchor's hue and saturation, a half at [`AGREEMENT_WIDTH`]
    /// from it: `1 / (1 + d² / σ²)`, written with one division.
    fn agreement(self, anchor: Chromaticity) -> f32 {
        let da = self.chromaticity.a - anchor.a;
        let db = self.chromaticity.b - anchor.b;
        let width_squared = AGREEMENT_WIDTH * AGREEMENT_WIDTH;
        width_squared / (width_squared + da * da + db * db)
    }

    /// What this colour's geometric weight is multiplied by at `strength`:
    /// 1 at strength 0, so the pass fades in from the plain mean.
    fn factor(self, anchor: Chromaticity, strength: f32) -> f32 {
        1.0 - strength + strength * self.agreement(anchor) * self.grey_factor
    }
}

/// A running weighted mean of colours.
#[derive(Debug, Default)]
struct WeightedMean {
    sum: LinearColor,
    weight: f32,
}

impl WeightedMean {
    fn add(&mut self, color: LinearColor, weight: f32) {
        self.sum += color * weight;
        self.weight += weight;
    }

    /// The mean, or `None` without weight or for a non-finite result.
    fn value(&self) -> Option<LinearColor> {
        let mean = self.sum / self.weight;
        let finite = [mean.r, mean.g, mean.b].iter().all(|c| c.is_finite());
        (self.weight > 0.0 && finite).then_some(mean)
    }
}

/// `selective` at the plain mean's Rec. 709 luminance, scaled no further than
/// its brightest channel reaching full. A selective colour with no light, or
/// a non-finite plain mean, gives the plain mean.
fn at_brightness_of(selective: LinearColor, plain: LinearColor) -> LinearColor {
    let (wanted, has) = (plain.luminance(), selective.luminance());
    if !(has > DARK && wanted.is_finite()) {
        return plain;
    }
    let peak = selective.r.max(selective.g).max(selective.b);
    let scale = wanted / has;
    // Dividing by the peak gives exactly 1 for it and at most 1 for the others.
    if peak * scale > 1.0 {
        selective / peak
    } else {
        selective * scale
    }
}

/// Group side in cells for groups at most `width` pixels wide, at least one
/// cell and at most `most`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "a finite, positive count clamped to the grid's columns"
)]
fn group_side(width: f32, cell_width: f32, most: usize) -> usize {
    let side = (width / cell_width).floor();
    if side >= 1.0 {
        (side as usize).clamp(1, most.max(1))
    } else {
        1
    }
}

impl ColorFocus {
    /// The sampler's geometry changed: rebuild the fields before the next frame.
    pub(super) fn invalidate(&mut self) {
        self.ready = false;
    }

    /// Builds the fields if the geometry changed, then this frame's group and
    /// block colours. `cell_colors` holds one colour per geometry cell.
    pub(super) fn prepare_frame(&mut self, geometry: &Geometry, cell_colors: &[LinearColor]) {
        if !self.ready {
            self.build(geometry, Sizes::SHIPPED);
        }
        self.groups.pool(cell_colors);
        self.blocks.pool(cell_colors);
    }

    /// Replaces each plain mean in `samples`, samples from `first`
    /// in strip order, by its focused colour. Call after
    /// [`Self::prepare_frame`].
    pub(super) fn refine(&self, first: usize, strength: f32, samples: &mut [LinearColor]) {
        let fields = &self.fields;
        // A picture has at least one cell, so at least one block.
        let far_rows = fields.far.chunks_exact(self.blocks.len()).skip(first);
        let near_rows = fields.starts.windows(2).skip(first);
        for (sample, (near, far)) in samples.iter_mut().zip(near_rows.zip(far_rows)) {
            *sample = self.focused(*sample, &fields.near[near[0]..near[1]], far, strength);
        }
    }

    fn focused(
        &self,
        plain: LinearColor,
        near: &[NearGroup],
        far: &[f32],
        strength: f32,
    ) -> LinearColor {
        let anchor = self.anchor(near).unwrap_or_else(|| Chromaticity::of(plain));
        let mut mean = WeightedMean::default();
        for near_group in near {
            let (color, tint) = self.groups.get(near_group.group);
            mean.add(color, near_group.weight * tint.factor(anchor, strength));
        }
        let blocks = self.blocks.colors.iter().zip(&self.blocks.tints);
        for (&weight, (&color, &tint)) in far.iter().zip(blocks) {
            mean.add(color, weight * tint.factor(anchor, strength));
        }
        mean.value()
            .map_or(plain, |selective| at_brightness_of(selective, plain))
    }

    /// The local lobe's mean over the near field, grey counted less; `None`
    /// for an empty near field.
    fn anchor(&self, near: &[NearGroup]) -> Option<Chromaticity> {
        let mut mean = WeightedMean::default();
        for near_group in near {
            let (color, tint) = self.groups.get(near_group.group);
            mean.add(color, near_group.local * tint.grey_factor);
        }
        mean.value().map(Chromaticity::of)
    }

    /// Pools cells into near groups and far blocks, and splits each sample's
    /// weights between its near groups and the far blocks.
    #[allow(
        clippy::cast_precision_loss,
        reason = "picture and grid sizes are far below f32's exact integer range"
    )]
    fn build(&mut self, geometry: &Geometry, sizes: Sizes) {
        let columns = geometry.columns.max(1);
        let cell_width = geometry.key.width as f32 / columns as f32;
        let lobe_width = geometry.radius / 4.0;
        let group = group_side(GROUP_LOBES * lobe_width, cell_width, sizes.group_cells);
        let block =
            (geometry.cells.len().div_ceil(sizes.pooled_blocks).isqrt()).max(BLOCK_GROUPS * group);
        self.groups.build(&geometry.cells, columns, group);
        self.blocks.build(&geometry.cells, columns, block);

        let near_radius = (sizes.near_height * geometry.key.height as f32)
            .min(sizes.near_radii_max * geometry.radius)
            .max(NEAR_RADII_MIN * geometry.radius)
            .max(NEAR_MIN_BLOCKS * cell_width * block as f32);
        let split = Split {
            cells: &geometry.cells,
            radius: geometry.radius,
            near_limit: (near_radius / geometry.radius).powi(2),
            groups: &self.groups,
            blocks: &self.blocks,
        };
        let mut near_weights = vec![None; self.groups.len()];
        self.fields.clear();
        let rows = geometry.weights.chunks_exact(geometry.cells.len());
        for (point, row) in geometry.key.sample_points(geometry.counts).zip(rows) {
            split.sample(point, row, &mut near_weights, &mut self.fields);
        }
        self.groups.in_use.fill(false);
        for near_group in &self.fields.near {
            self.groups.in_use[near_group.group] = true;
        }
        self.ready = true;
    }
}

impl Fields {
    fn clear(&mut self) {
        self.starts.clear();
        self.starts.push(0);
        self.near.clear();
        self.far.clear();
    }
}

/// Splits samples' geometric weights between near groups and far blocks.
struct Split<'a> {
    cells: &'a [[u32; 4]],
    radius: f32,
    /// The near field's squared radius, in kernel radii.
    near_limit: f32,
    groups: &'a Pooling,
    blocks: &'a Pooling,
}

impl Split<'_> {
    /// Appends the sample at `point`, with geometric weights `row` (one per
    /// cell), to `fields`.
    fn sample(
        &self,
        point: [f32; 2],
        row: &[f32],
        near_weights: &mut [Option<(f32, f32)>],
        fields: &mut Fields,
    ) {
        for (weight, &rect) in near_weights.iter_mut().zip(&self.groups.rects) {
            *weight = (distance_squared(rect, point, self.radius) <= self.near_limit)
                .then_some((0.0, 0.0));
        }
        let far_start = fields.far.len();
        fields.far.resize(far_start + self.blocks.len(), 0.0);
        let far = &mut fields.far[far_start..];
        let pooled = self.groups.of_cell.iter().zip(&self.blocks.of_cell);
        for ((&cell, &weight), (&(group, _), &(block, _))) in self.cells.iter().zip(row).zip(pooled)
        {
            if let Some((sum, local)) = &mut near_weights[group] {
                *sum += weight;
                *local += cell_area(cell) * local_lobe(distance_squared(cell, point, self.radius));
            } else {
                far[block] += weight;
            }
        }
        fields.near.extend(
            near_weights
                .iter()
                .enumerate()
                .filter_map(|(group, &weights)| {
                    weights.map(|(weight, local)| NearGroup {
                        group,
                        weight,
                        local,
                    })
                }),
        );
        fields.starts.push(fields.near.len());
    }
}

#[cfg(test)]
mod tests;
