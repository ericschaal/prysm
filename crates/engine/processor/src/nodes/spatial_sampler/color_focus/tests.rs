use super::super::tests::{Configured, picture, strips};
use super::*;
use crate::frames::{FrameView, Viewport};
use crate::nodes::SpatialSampler;
use prysm_core::{ColorFocusPercent, EdgeColors, SampleDensity};

type Rgb = [u8; 3];

/// A small deterministic generator (xorshift64*), so pictures are seeded.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn byte(&mut self) -> u8 {
        self.next().to_le_bytes()[7]
    }

    fn rgb(&mut self) -> Rgb {
        [self.byte(), self.byte(), self.byte()]
    }
}

fn sampler(focus: u8, reach_percent: u8) -> Configured {
    let mut sampler = Configured::new(SampleDensity(128), reach_percent);
    sampler.set_color_focus(ColorFocusPercent(focus));
    sampler
}

fn samples(colors: &EdgeColors) -> Vec<LinearColor> {
    strips(colors)
        .into_iter()
        .flat_map(|strip| strip.samples().to_vec())
        .collect()
}

fn oklab_distance(a: LinearColor, b: LinearColor) -> f32 {
    Oklab::from_linear(a).distance(Oklab::from_linear(b))
}

fn hue_degrees(color: LinearColor) -> f32 {
    let Oklab { a, b, .. } = Oklab::from_linear(color);
    b.atan2(a).to_degrees()
}

fn hue_difference(a: LinearColor, b: LinearColor) -> f32 {
    let difference = (hue_degrees(a) - hue_degrees(b)).rem_euclid(360.0);
    difference.min(360.0 - difference)
}

fn chroma(color: LinearColor) -> f32 {
    Chromaticity::of(color).chroma()
}

fn linear([r, g, b]: Rgb) -> LinearColor {
    LinearColor::from_srgb(prysm_core::Color::new(r, g, b))
}

fn percentile(values: &mut [f32], share: f32) -> f32 {
    values.sort_by(f32::total_cmp);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "an index into a short list of measurements"
    )]
    let index = ((share * values.len() as f32) as usize).min(values.len().saturating_sub(1));
    values.get(index).copied().unwrap_or_default()
}

// ── Pictures (640x360 unless stated) ────────────────────────────────────────

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;

fn halves(left: Rgb, right: Rgb) -> FrameView {
    picture(
        WIDTH,
        HEIGHT,
        |x, _| if x < WIDTH / 2 { left } else { right },
    )
}

fn opposing() -> FrameView {
    halves([230, 80, 20], [20, 90, 230])
}

/// A magenta square at the top and a green disc at the left, on mid grey.
fn objects_on_grey() -> FrameView {
    picture(WIDTH, HEIGHT, |x, y| {
        let (dx, dy) = (f64::from(x) - 40.0, f64::from(y) - 180.0);
        if (280..360).contains(&x) && y < 60 {
            [220, 30, 200]
        } else if dx * dx + dy * dy < 40.0 * 40.0 {
            [40, 200, 60]
        } else {
            [128, 128, 128]
        }
    })
}

/// Hue across, lightness down.
fn gradients() -> FrameView {
    picture(WIDTH, HEIGHT, |x, y| {
        let hue = f64::from(x) / f64::from(WIDTH) * 6.0;
        let light = 1.0 - 0.7 * f64::from(y) / f64::from(HEIGHT);
        let channel = |offset: f64| {
            let distance = ((hue - offset).rem_euclid(6.0) - 3.0).abs();
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a clamped 0-255 channel"
            )]
            let value = ((distance - 1.0).clamp(0.0, 1.0) * light * 255.0).round() as u8;
            value
        };
        [channel(0.0), channel(2.0), channel(4.0)]
    })
}

fn random_blocks(seed: u64, side: u32, dark: bool) -> FrameView {
    let columns = WIDTH.div_ceil(side);
    let mut random = Random(seed);
    let colors: Vec<Rgb> = (0..columns * HEIGHT.div_ceil(side))
        .map(|_| {
            let color = random.rgb();
            if dark { color.map(|c| c / 6) } else { color }
        })
        .collect();
    picture(WIDTH, HEIGHT, |x, y| {
        colors[usize::try_from(y / side * columns + x / side).unwrap()]
    })
}

/// A dark scene: deep blue night with a dim orange window near the bottom.
fn dark_scene() -> FrameView {
    picture(WIDTH, HEIGHT, |x, y| {
        if (420..470).contains(&x) && (300..340).contains(&y) {
            [90, 50, 10]
        } else {
            [4, 6, 18 + (x / 64).to_le_bytes()[0]]
        }
    })
}

fn accuracy_pictures() -> Vec<(&'static str, FrameView)> {
    vec![
        ("Opposing halves", opposing()),
        ("Objects on grey", objects_on_grey()),
        ("Hue and lightness gradients", gradients()),
        ("Random blocks, 16 px", random_blocks(7, 16, false)),
        ("Random blocks, 48 px", random_blocks(11, 48, false)),
        ("Dark scene", dark_scene()),
        ("Dark random blocks, 16 px", random_blocks(13, 16, true)),
    ]
}

// ── The reference: the same formula on every cell ──────────────────────────

/// Each sample's focused colour with every cell at full resolution, from the
/// sampler's last frame; `plain` holds its plain means in strip order.
fn reference(sampler: &SpatialSampler, plain: &[LinearColor], strength: f32) -> Vec<LinearColor> {
    let cells = &sampler.cells;
    let colors = &sampler.cell_colors;
    let tints: Vec<Tint> = colors.iter().map(|&color| Tint::of(color)).collect();
    let points = sampler.sample_points.iter().flatten();
    let rows = sampler
        .sample_weights
        .iter()
        .flat_map(|weights| weights.chunks_exact(cells.len()));
    points
        .zip(rows)
        .zip(plain)
        .map(|((&point, row), &plain)| {
            let mut anchor = WeightedMean::default();
            for ((&cell, &color), tint) in cells.iter().zip(colors).zip(&tints) {
                let distance_squared = distance_squared(cell, point, sampler.radius);
                let local = cell_area(cell) * local_lobe(distance_squared);
                anchor.add(color, local * tint.grey_factor);
            }
            let anchor = anchor
                .value()
                .map_or(Chromaticity::of(plain), Chromaticity::of);
            let mut mean = WeightedMean::default();
            for ((&weight, &color), tint) in row.iter().zip(colors).zip(&tints) {
                mean.add(color, weight * tint.factor(anchor, strength));
            }
            mean.value()
                .map_or(plain, |selective| at_brightness_of(selective, plain))
        })
        .collect()
}

fn rebuild(sampler: &mut Configured, sizes: Sizes) {
    let sampler: &mut SpatialSampler = sampler;
    let key = sampler.built_for.expect("a processed frame");
    let (width, height) = (key.width, key.height);
    let geometry = Geometry {
        width,
        height,
        cells: &sampler.cells,
        columns: sampler.columns,
        radius: sampler.radius,
        points: &sampler.sample_points,
        weights: &sampler.sample_weights,
    };
    sampler.focus.build(&geometry, sizes);
}

/// Per-LED distance between the two-part evaluation at `sizes` and the
/// reference, in just-noticeable steps.
fn errors(view: &FrameView, focus: u8, reach: u8, sizes: Sizes) -> Vec<f32> {
    let plain = samples(&sampler(0, reach).process(view));
    let mut focused = sampler(focus, reach);
    let _ = focused.process(view);
    rebuild(&mut focused, sizes);
    let actual = samples(&focused.process(view));
    let expected = reference(&focused, &plain, f32::from(focus) / 100.0);
    actual
        .into_iter()
        .zip(expected)
        .map(|(a, e)| oklab_distance(a, e) / Oklab::JUST_NOTICEABLE)
        .collect()
}

#[test]
fn the_two_part_evaluation_agrees_with_every_cell_at_full_resolution() {
    // A blank line first: the test harness has started a line of its own.
    eprintln!();
    eprintln!("| Picture | Focus | Mean | p95 | Max |");
    eprintln!("| --- | ---: | ---: | ---: | ---: |");
    for (name, view) in accuracy_pictures() {
        for focus in [50, 100] {
            let mut errors = errors(&view, focus, 10, Sizes::SHIPPED);
            let mean = errors.iter().sum::<f32>() / errors.len() as f32;
            let p95 = percentile(&mut errors, 0.95);
            let max = percentile(&mut errors, 1.0);
            eprintln!("| {name} | {focus} | {mean:.2} | {p95:.2} | {max:.2} |");
            assert!(p95 < 1.0, "{name} at {focus}: p95 {p95} steps");
        }
    }
}

/// The study's table of evaluation sizes: at each edge depth, the colours
/// each sample weighs (near groups and blocks, the cost) and the worst p95 and
/// maximum error over the accuracy pictures at focus 100, for the shipped
/// sizes and one change at a time.
#[test]
#[ignore = "prints a table for the selective-sampler study; run with --ignored --nocapture"]
fn evaluation_sizes_table() {
    let shipped = Sizes::SHIPPED;
    let variants = [
        ("shipped", shipped),
        (
            "near field to 0.1 of the height",
            Sizes {
                near_height: 0.1,
                ..shipped
            },
        ),
        (
            "near field to 0.3 of the height",
            Sizes {
                near_height: 0.3,
                ..shipped
            },
        ),
        (
            "near field at most 1 radius",
            Sizes {
                near_radii_max: 1.0,
                ..shipped
            },
        ),
        (
            "groups up to 3 cells",
            Sizes {
                group_cells: 3,
                ..shipped
            },
        ),
        (
            "single cells only",
            Sizes {
                group_cells: 1,
                ..shipped
            },
        ),
        (
            "400 blocks",
            Sizes {
                pooled_blocks: 400,
                ..shipped
            },
        ),
        (
            "25 blocks",
            Sizes {
                pooled_blocks: 25,
                ..shipped
            },
        ),
    ];
    eprintln!();
    eprintln!("| Edge depth | Sizes | Colours per sample | Worst p95 | Worst max |");
    eprintln!("| ---: | --- | ---: | ---: | ---: |");
    let pictures = accuracy_pictures();
    for reach in [1, 5, 10, 20, 30, 40, 60, 100] {
        for (name, sizes) in variants {
            let (mut worst_p95, mut worst_max) = (0.0_f32, 0.0_f32);
            for (_, view) in &pictures {
                let mut errors = errors(view, 100, reach, sizes);
                worst_p95 = worst_p95.max(percentile(&mut errors, 0.95));
                worst_max = worst_max.max(percentile(&mut errors, 1.0));
            }
            let mut probe = sampler(100, reach);
            let _ = probe.process(&pictures[0].1);
            rebuild(&mut probe, sizes);
            let fields = &probe.focus.fields;
            let samples: usize = fields.iter().map(|f| f.starts.len() - 1).sum();
            let near: usize = fields.iter().map(|f| f.near.len()).sum();
            let colours = near as f32 / samples as f32 + probe.focus.blocks.len() as f32;
            eprintln!("| {reach} % | {name} | {colours:.0} | {worst_p95:.2} | {worst_max:.2} |");
        }
    }
}

// ── Behaviour ───────────────────────────────────────────────────────────────

/// Small pictures for the property tests, cheap in a debug build.
fn varied_pictures(width: u32, height: u32) -> Vec<FrameView> {
    let mut pictures = vec![
        picture(width, height, |_, _| [0, 0, 0]),
        picture(width, height, |_, _| [255, 255, 255]),
        picture(width, height, |x, y| {
            [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]]
                [usize::from(x * 2 >= width) + 2 * usize::from(y * 2 >= height)]
        }),
        picture(width, height, |x, y| {
            if x == width / 3 && y == height / 2 {
                [255, 255, 255]
            } else {
                [0, 0, 0]
            }
        }),
        picture(width, height, |x, _| {
            if x == 0 { [0, 0, 255] } else { [255, 255, 0] }
        }),
    ];
    for seed in [1, 2, 3] {
        let mut random = Random(seed);
        let pixels: Vec<Rgb> = (0..width * height).map(|_| random.rgb()).collect();
        pictures.push(picture(width, height, |x, y| {
            pixels[usize::try_from(y * width + x).unwrap()]
        }));
        let mut random = Random(seed + 10);
        let blocks: Vec<Rgb> = (0..16).map(|_| random.rgb()).collect();
        pictures.push(picture(width, height, |x, y| {
            blocks[usize::try_from((y * 4 / height) * 4 + x * 4 / width).unwrap()]
        }));
    }
    pictures
}

#[test]
fn focus_zero_is_the_plain_mean_exactly_even_after_focusing() {
    for (width, height) in [(96, 54), (33, 17), (1, 1)] {
        for view in varied_pictures(width, height) {
            let expected = Configured::new(SampleDensity(128), 10).process(&view);
            let mut sampler = sampler(100, 10);
            let _ = sampler.process(&view);
            sampler.set_color_focus(ColorFocusPercent(0));
            assert_eq!(sampler.process(&view), expected);
        }
    }
}

#[test]
fn emitted_light_stays_in_range_and_keeps_the_plain_brightness() {
    // Every focus at the default edge depth and on a one-pixel picture; a few
    // at the extreme edge depths.
    let every: Vec<u8> = (1..=100).collect();
    let few = vec![1, 2, 50, 99, 100];
    for (width, height, reach, focuses) in [
        (96, 54, 10, &every),
        (96, 54, 1, &few),
        (64, 64, 100, &few),
        (1, 1, 10, &every),
    ] {
        for view in varied_pictures(width, height) {
            let plain = samples(&sampler(0, reach).process(&view));
            let mut focused = sampler(1, reach);
            for &focus in focuses {
                focused.set_color_focus(ColorFocusPercent(focus));
                let output = samples(&focused.process(&view));
                for (actual, plain) in output.into_iter().zip(&plain) {
                    let channels = [actual.r, actual.g, actual.b];
                    assert!(
                        channels
                            .iter()
                            .all(|c| c.is_finite() && (0.0..=1.0).contains(c)),
                        "{focus}: {actual:?}"
                    );
                    let (wanted, emitted) = (plain.luminance(), actual.luminance());
                    let kept = (emitted - wanted).abs() <= 1e-5 + wanted * 1e-4;
                    let capped = emitted < wanted && channels.contains(&1.0);
                    assert!(kept || capped, "{focus}: {actual:?} against {plain:?}");
                }
            }
        }
    }
}

#[test]
fn brightness_follows_the_plain_mean_and_is_capped_at_full() {
    let red = LinearColor::new(0.5, 0.0, 0.0);
    let grey = LinearColor::new(0.2, 0.2, 0.2);
    let scaled = at_brightness_of(red, grey);
    assert!((scaled.luminance() - grey.luminance()).abs() < 1e-6);
    // Red at white's luminance would need five times full red: capped at full.
    let white = LinearColor::new(1.0, 1.0, 1.0);
    assert_eq!(
        at_brightness_of(red, white),
        LinearColor::new(1.0, 0.0, 0.0)
    );
    // Nothing to scale, or nothing to scale to: the plain mean.
    assert_eq!(at_brightness_of(LinearColor::black(), grey), grey);
    let nan = LinearColor::new(f32::NAN, 0.0, 0.0);
    assert_eq!(at_brightness_of(nan, grey), grey);
    assert_eq!(
        at_brightness_of(red, LinearColor::black()),
        LinearColor::black()
    );
    assert!(WeightedMean::default().value().is_none());
}

#[test]
fn without_near_cells_or_weight_a_sample_keeps_its_plain_mean() {
    let focus = ColorFocus::default();
    assert!(focus.anchor(&[]).is_none());
    let plain = LinearColor::new(0.3, 0.2, 0.1);
    assert_eq!(focus.focused(plain, &[], &[], 1.0), plain);
}

#[test]
#[should_panic(expected = "index out of bounds")]
fn a_near_group_outside_the_frames_colours_is_a_bug() {
    let stray = NearGroup {
        group: 7,
        weight: 1.0,
        local: 1.0,
    };
    let _ = ColorFocus::default().focused(LinearColor::black(), &[stray], &[], 1.0);
}

#[test]
fn a_uniform_picture_is_the_same_at_any_focus() {
    for rgb in [[0, 0, 0], [255, 255, 255], [200, 40, 120], [30, 30, 30]] {
        let view = picture(96, 54, |_, _| rgb);
        let plain = samples(&sampler(0, 10).process(&view));
        for focus in [1, 30, 100] {
            for (actual, expected) in samples(&sampler(focus, 10).process(&view))
                .iter()
                .zip(&plain)
            {
                assert!(
                    oklab_distance(*actual, *expected) < 1e-4,
                    "{focus}: {actual:?}"
                );
            }
        }
    }
}

#[test]
fn lights_beside_each_half_take_its_hue_as_focus_rises() {
    let (left, right) = ([230, 80, 20], [20, 90, 230]);
    let view = halves(left, right);
    let at = |focus| sampler(focus, 10).process(&view);
    // The middle of the side strips, and the top strip just beside the boundary.
    let beside = |colors: &EdgeColors| {
        [
            (colors.left.sample_at(0.5), linear(left)),
            (colors.right.sample_at(0.5), linear(right)),
            (colors.top.sample_at(0.4), linear(left)),
            (colors.top.sample_at(0.6), linear(right)),
        ]
    };
    let mut previous = [f32::INFINITY; 4];
    for focus in [0, 25, 50, 75, 100] {
        let colors = at(focus);
        for ((led, half), previous) in beside(&colors).into_iter().zip(&mut previous) {
            let difference = hue_difference(led, half);
            assert!(
                difference < *previous,
                "{focus}: {difference} after {previous}"
            );
            *previous = difference;
        }
    }
    // Beside the boundary the plain mean is visibly mixed; focus mostly undoes it.
    let plain = hue_difference(at(0).top.sample_at(0.4), linear(left));
    let focused = hue_difference(at(100).top.sample_at(0.4), linear(left));
    assert!(focused < plain / 2.0, "{focused} against {plain}");
}

#[test]
fn grey_next_to_a_saturated_object_keeps_the_objects_hue() {
    let view = objects_on_grey();
    let (plain, focused) = (
        sampler(0, 10).process(&view).left.sample_at(0.5),
        sampler(100, 10).process(&view).left.sample_at(0.5),
    );
    let green = linear([40, 200, 60]);
    assert!(hue_difference(focused, green) < 10.0, "{focused:?}");
    assert!(
        chroma(focused) > chroma(plain) * 1.2,
        "{focused:?} against {plain:?}"
    );
}

#[test]
fn a_small_change_of_the_picture_moves_the_lights_a_little() {
    // A boundary right beside the top strip's middle LEDs, moved by a pixel.
    let at = |boundary: u32, focus| {
        let view = picture(WIDTH, HEIGHT, |x, _| {
            if x < boundary {
                [230, 80, 20]
            } else {
                [20, 90, 230]
            }
        });
        samples(&sampler(focus, 10).process(&view))
    };
    let largest_change = |focus| {
        at(320, focus)
            .into_iter()
            .zip(at(321, focus))
            .map(|(a, b)| oklab_distance(a, b) / Oklab::JUST_NOTICEABLE)
            .fold(0.0_f32, f32::max)
    };
    let (plain, focused) = (largest_change(0), largest_change(100));
    // The LED at the boundary sees half a step move in the plain mean. Focus
    // sharpens the boundary, so the same move counts more (3.2 times here, as
    // much as the formula on every cell gives), but boundedly.
    assert!(plain > 0.3, "{plain} steps");
    assert!(focused < plain * 4.0, "{focused} against {plain} steps");
}

#[test]
fn a_new_geometry_rebuilds_the_fields_and_frames_reuse_their_storage() {
    let mut sampler = sampler(50, 10);
    let view = random_blocks(5, 16, false);
    let _ = sampler.process(&view);
    let near = sampler.focus.fields[0].near.as_ptr();
    let tints = sampler.focus.groups.tints.as_ptr();
    let blocks = sampler.focus.blocks.colors.as_ptr();
    let before = sampler.focus.fields[0].near.len();
    let _ = sampler.process(&random_blocks(6, 16, false));
    assert_eq!(sampler.focus.fields[0].near.as_ptr(), near, "fields kept");
    assert_eq!(
        sampler.focus.groups.tints.as_ptr(),
        tints,
        "group colours reused"
    );
    assert_eq!(
        sampler.focus.blocks.colors.as_ptr(),
        blocks,
        "block colours reused"
    );

    // A crop changes the geometry: the fields match a fresh sampler's.
    let mut cropped = view.clone();
    cropped.viewport = Viewport {
        x: 0,
        y: 45,
        width: WIDTH,
        height: 270,
    };
    let _ = sampler.process(&cropped);
    let mut fresh = super::tests::sampler(50, 10);
    let _ = fresh.process(&cropped);
    assert_ne!(sampler.focus.fields[0].near.len(), before);
    assert_eq!(sampler.focus.fields, fresh.focus.fields);
    // A changed edge depth too.
    sampler.set_reach(30);
    let _ = sampler.process(&cropped);
    let mut deeper = super::tests::sampler(50, 30);
    let _ = deeper.process(&cropped);
    assert_eq!(sampler.focus.fields, deeper.focus.fields);
}
