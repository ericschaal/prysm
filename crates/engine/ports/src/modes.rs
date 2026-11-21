//! What a capture source can deliver: its sizes and rates, as the hardware
//! that owns them says.
use crate::ValidationError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::num::NonZeroU32;

/// The values `min`, `min + step`, … up to `max`; a single value is
/// `min == max`. `max` is always reachable from `min`.
///
/// Serialized as `{ "min": 2, "max": 1920, "step": 2 }`; one whose `max` is
/// not reachable, a zero step or `min > max` does not deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "SpanFields", into = "SpanFields")]
pub struct Span {
    min: u32,
    max: u32,
    step: NonZeroU32,
}

impl Span {
    #[must_use]
    pub const fn exactly(value: u32) -> Self {
        Self {
            min: value,
            max: value,
            step: NonZeroU32::MIN,
        }
    }

    /// `min` to `max` in steps of `step`, `max` lowered to the last value
    /// reached (and the step to 1 for a single value). `None` for a zero step or `min > max`.
    #[must_use]
    pub const fn new(min: u32, max: u32, step: u32) -> Option<Self> {
        let Some(step) = NonZeroU32::new(step) else {
            return None;
        };
        if min > max {
            return None;
        }
        let max = max - (max - min) % step.get();
        // One value has one spelling.
        let step = if min == max { NonZeroU32::MIN } else { step };
        Some(Self { min, max, step })
    }

    #[must_use]
    pub const fn min(self) -> u32 {
        self.min
    }

    #[must_use]
    pub const fn max(self) -> u32 {
        self.max
    }

    #[must_use]
    pub const fn step(self) -> NonZeroU32 {
        self.step
    }

    #[must_use]
    pub const fn contains(&self, value: u32) -> bool {
        self.min <= value && value <= self.max && (value - self.min).is_multiple_of(self.step.get())
    }

    /// The values in both, `None` if there are none. Their step is the
    /// least common multiple of both steps.
    #[must_use]
    pub fn and(&self, other: &Self) -> Option<Self> {
        let (low, high) = (self.min.max(other.min), self.max.min(other.max));
        if low > high {
            return None;
        }
        // x ≡ self.min (mod s) and x ≡ other.min (mod t).
        let (s, t) = (i128::from(self.step.get()), i128::from(other.step.get()));
        let g = gcd(s, t);
        let difference = i128::from(other.min) - i128::from(self.min);
        if difference % g != 0 {
            return None;
        }
        // s·k ≡ difference (mod t), solved with the inverse of s/g modulo t/g.
        let modulus = t / g;
        let (_, inverse, _) = extended_gcd(s / g, modulus);
        let k = ((difference / g) % modulus * inverse).rem_euclid(modulus);
        let common = i128::from(self.min) + s * k;
        let step = s / g * t;
        let first = i128::from(low) + (common - i128::from(low)).rem_euclid(step);
        let first = u32::try_from(first).ok().filter(|first| *first <= high)?;
        match u32::try_from(step) {
            Ok(step) => Self::new(first, high, step),
            // A step beyond `u32` reaches no second value.
            Err(_) => Some(Self::exactly(first)),
        }
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// `(g, x, y)` with `a·x + b·y = g = gcd(a, b)`.
fn extended_gcd(a: i128, b: i128) -> (i128, i128, i128) {
    if b == 0 {
        (a, 1, 0)
    } else {
        let (g, x, y) = extended_gcd(b, a % b);
        (g, y, x - a / b * y)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.min == self.max, self.step.get()) {
            (true, _) => write!(f, "{}", self.min),
            (false, 1) => write!(f, "{} to {}", self.min, self.max),
            (false, step) => write!(f, "{} to {} in steps of {step}", self.min, self.max),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpanFields {
    min: u32,
    max: u32,
    step: u32,
}

impl TryFrom<SpanFields> for Span {
    type Error = String;

    fn try_from(SpanFields { min, max, step }: SpanFields) -> Result<Self, String> {
        Self::new(min, max, step)
            .filter(|span| span.max == max && (min < max || step == 1))
            .ok_or_else(|| format!("{min} to {max} in steps of {step} is not a span"))
    }
}

impl From<Span> for SpanFields {
    fn from(span: Span) -> Self {
        Self {
            min: span.min,
            max: span.max,
            step: span.step.get(),
        }
    }
}

/// One way a source delivers: every combination of these widths, heights
/// and frame rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mode {
    width: Span,
    height: Span,
    fps: Span,
}

impl Mode {
    #[must_use]
    pub const fn new(width: Span, height: Span, fps: Span) -> Self {
        Self { width, height, fps }
    }

    #[must_use]
    pub const fn width(&self) -> Span {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> Span {
        self.height
    }

    #[must_use]
    pub const fn fps(&self) -> Span {
        self.fps
    }

    #[must_use]
    pub const fn contains(&self, width: u32, height: u32, fps: u32) -> bool {
        self.width.contains(width) && self.height.contains(height) && self.fps.contains(fps)
    }

    /// What both deliver, axis by axis; `None` if an axis has nothing in common.
    #[must_use]
    pub fn and(&self, other: &Self) -> Option<Self> {
        Some(Self {
            width: self.width.and(&other.width)?,
            height: self.height.and(&other.height)?,
            fps: self.fps.and(&other.fps)?,
        })
    }
}

/// What a source can deliver with the settings it was asked about.
///
/// Whoever owns the hardware answers for it, by one rule: a [`Span`]
/// wherever the hardware accepts any value in it, whether it says so itself
/// (a V4L2 stepwise size, an `AVFoundation` rate range) or is known to
/// scale to what is asked (LG's capture); concrete modes only where the
/// hardware offers fixed ones. Unknown only when the hardware cannot be
/// reached or asked.
///
/// Serialized as `{ "status": "unknown" }` or
/// `{ "status": "known", "modes": [...] }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "ModesFields", into = "ModesFields")]
pub enum Modes {
    /// The source cannot say (it cannot reach its hardware); only opening tells.
    Unknown,
    /// Exactly these; empty: nothing with these settings.
    Known(Vec<Mode>),
}

impl From<Mode> for Modes {
    fn from(mode: Mode) -> Self {
        Self::Known(vec![mode])
    }
}

/// The most values a refusal lists.
const LISTED: usize = 6;

impl Modes {
    /// Whether a mode delivers `width`×`height` at `fps`; [`Modes::Unknown`]
    /// passes.
    ///
    /// # Errors
    /// The first wrong field (`width`, else `height`, else `fps`, relative to
    /// the section) with what is delivered instead, the reason naming the
    /// field in words, since the portal shows the reason without the field.
    pub fn check(&self, width: u32, height: u32, fps: u32) -> Result<(), ValidationError> {
        let Self::Known(modes) = self else {
            return Ok(());
        };
        if modes.is_empty() {
            return Err(ValidationError::new(
                "width",
                "this source delivers no picture with these settings",
            ));
        }
        let wide: Vec<&Mode> = modes.iter().filter(|m| m.width.contains(width)).collect();
        if wide.is_empty() {
            return Err(ValidationError::new(
                "width",
                format!(
                    "this source delivers a width of {}",
                    listed(modes.iter().map(Mode::width))
                ),
            ));
        }
        let sized: Vec<&Mode> = wide
            .iter()
            .copied()
            .filter(|m| m.height.contains(height))
            .collect();
        if sized.is_empty() {
            return Err(ValidationError::new(
                "height",
                format!(
                    "at width {width} this source delivers a height of {}",
                    listed(wide.iter().map(|m| m.height))
                ),
            ));
        }
        if sized.iter().any(|m| m.fps.contains(fps)) {
            return Ok(());
        }
        Err(ValidationError::new(
            "fps",
            format!(
                "at {width}x{height} this source delivers a frame rate of {}",
                listed(sized.iter().map(|m| m.fps))
            ),
        ))
    }
}

/// `a, b or c`, sorted and without repeats, at most [`LISTED`] of them.
fn listed(spans: impl Iterator<Item = Span>) -> String {
    let spans: Vec<String> = spans
        .collect::<BTreeSet<_>>()
        .iter()
        .map(Span::to_string)
        .collect();
    let more = spans.len().saturating_sub(LISTED);
    let (shown, last) = if more > 0 {
        (&spans[..LISTED], format!("{more} more"))
    } else {
        (&spans[..spans.len() - 1], spans[spans.len() - 1].clone())
    };
    if shown.is_empty() {
        last
    } else {
        format!("{} or {last}", shown.join(", "))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum ModesFields {
    Unknown,
    Known { modes: Vec<Mode> },
}

impl From<ModesFields> for Modes {
    fn from(fields: ModesFields) -> Self {
        match fields {
            ModesFields::Unknown => Self::Unknown,
            ModesFields::Known { modes } => Self::Known(modes),
        }
    }
}

impl From<Modes> for ModesFields {
    fn from(modes: Modes) -> Self {
        match modes {
            Modes::Unknown => Self::Unknown,
            Modes::Known(modes) => Self::Known { modes },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn span(min: u32, max: u32, step: u32) -> Span {
        Span::new(min, max, step).unwrap()
    }

    fn values(span: Option<Span>, within: u32) -> Vec<u32> {
        (0..=within)
            .filter(|v| span.is_some_and(|s| s.contains(*v)))
            .collect()
    }

    #[test]
    fn spans_reach_their_max_and_refuse_nonsense() {
        assert_eq!(span(2, 1921, 2).max(), 1920);
        assert_eq!(span(5, 5, 7), Span::exactly(5));
        assert_eq!(Span::new(3, 2, 1), None);
        assert_eq!(Span::new(1, 2, 0), None);
        let even = span(2, 10, 2);
        assert!(even.contains(2) && even.contains(10) && even.contains(6));
        assert!(!even.contains(0) && !even.contains(7) && !even.contains(12));
        assert_eq!(even.to_string(), "2 to 10 in steps of 2");
        assert_eq!(span(1, 30, 1).to_string(), "1 to 30");
        assert_eq!(Span::exactly(640).to_string(), "640");
    }

    #[test]
    fn intersections_match_brute_force() {
        let spans = [
            span(0, 60, 1),
            span(2, 60, 2),
            span(1, 59, 2),
            span(3, 57, 6),
            span(16, 50, 16),
            span(5, 45, 10),
            span(7, 7, 1),
            span(10, 20, 4),
        ];
        for a in spans {
            for b in spans {
                let both = a.and(&b);
                let expected: Vec<u32> = (0..=60)
                    .filter(|v| a.contains(*v) && b.contains(*v))
                    .collect();
                assert_eq!(values(both, 60), expected, "{a} and {b}");
                assert_eq!(both, b.and(&a), "{a} and {b} commute");
                if let Some(both) = both {
                    assert!(both.contains(both.max()));
                }
            }
        }
        // Encoder limits against capture limits.
        assert_eq!(
            span(2, 1920, 2).and(&span(176, 1920, 2)),
            Some(span(176, 1920, 2))
        );
        assert_eq!(span(2, 1920, 2).and(&span(1, 1919, 2)), None);
        assert_eq!(span(1, 10, 1).and(&span(11, 20, 1)), None);
        // Huge steps meet at one value at most.
        let a = span(0, u32::MAX, u32::MAX);
        let b = span(1, u32::MAX, u32::MAX - 1);
        assert_eq!(values(a.and(&b), 10), Vec::<u32>::new());
        assert_eq!(
            a.and(&span(u32::MAX, u32::MAX, 1)),
            Some(Span::exactly(u32::MAX))
        );
    }

    #[test]
    fn modes_intersect_per_axis() {
        let capture = Mode::new(span(2, 1920, 2), span(2, 1080, 2), span(1, 60, 1));
        let encoder = Mode::new(span(176, 1920, 2), span(144, 1088, 2), span(1, 30, 1));
        let both = capture.and(&encoder).unwrap();
        assert_eq!(
            both,
            Mode::new(span(176, 1920, 2), span(144, 1080, 2), span(1, 30, 1))
        );
        assert!(both.contains(640, 360, 30) && !both.contains(640, 360, 31));
        let tiny = Mode::new(Span::exactly(8), Span::exactly(8), Span::exactly(30));
        assert_eq!(tiny.and(&encoder), None);
    }

    #[test]
    fn checks_name_the_first_wrong_field_and_what_is_offered() {
        assert_eq!(Modes::Unknown.check(0, 0, 0), Ok(()));
        let fixed = |w, h, fps| Mode::new(Span::exactly(w), Span::exactly(h), fps);
        let modes = Modes::Known(vec![
            fixed(640, 360, span(1, 30, 1)),
            fixed(1280, 720, span(1, 30, 1)),
            fixed(1280, 720, Span::exactly(60)),
            fixed(1920, 1080, span(5, 30, 5)),
        ]);
        assert_eq!(modes.check(1280, 720, 60), Ok(()));
        let error = |w, h, fps| modes.check(w, h, fps).unwrap_err();
        assert_eq!(
            error(800, 360, 30),
            ValidationError::new("width", "this source delivers a width of 640, 1280 or 1920")
        );
        assert_eq!(
            error(1280, 360, 30),
            ValidationError::new(
                "height",
                "at width 1280 this source delivers a height of 720"
            )
        );
        assert_eq!(
            error(1280, 720, 45),
            ValidationError::new(
                "fps",
                "at 1280x720 this source delivers a frame rate of 1 to 30 or 60"
            )
        );
        assert_eq!(
            error(1920, 1080, 12).reason,
            "at 1920x1080 this source delivers a frame rate of 5 to 30 in steps of 5"
        );
        let many = Modes::Known(
            (1..=9)
                .map(|w| fixed(w * 100, 100, Span::exactly(30)))
                .collect(),
        );
        assert_eq!(
            many.check(50, 100, 30).unwrap_err().reason,
            "this source delivers a width of 100, 200, 300, 400, 500, 600 or 3 more"
        );
        assert_eq!(
            Modes::Known(Vec::new())
                .check(640, 360, 30)
                .unwrap_err()
                .field,
            "width"
        );
    }

    #[test]
    fn serialization_is_tagged_and_spans_are_validated() {
        let modes = Modes::from(Mode::new(
            span(2, 1920, 2),
            Span::exactly(360),
            span(1, 60, 1),
        ));
        let value = serde_json::to_value(&modes).unwrap();
        assert_eq!(
            value,
            json!({ "status": "known", "modes": [{
                "width": { "min": 2, "max": 1920, "step": 2 },
                "height": { "min": 360, "max": 360, "step": 1 },
                "fps": { "min": 1, "max": 60, "step": 1 },
            }] })
        );
        assert_eq!(serde_json::from_value::<Modes>(value).unwrap(), modes);
        assert_eq!(
            serde_json::to_value(Modes::Unknown).unwrap(),
            json!({ "status": "unknown" })
        );
        assert_eq!(
            serde_json::from_value::<Modes>(json!({ "status": "unknown" })).unwrap(),
            Modes::Unknown
        );
        for bad in [
            json!({ "min": 2, "max": 5, "step": 2 }),
            json!({ "min": 2, "max": 1, "step": 1 }),
            json!({ "min": 2, "max": 2, "step": 0 }),
            json!({ "min": 2, "max": 2, "step": 1, "extra": 1 }),
        ] {
            assert!(
                serde_json::from_value::<Span>(bad.clone()).is_err(),
                "{bad}"
            );
        }
        assert!(serde_json::from_value::<Modes>(json!({ "status": "maybe" })).is_err());
    }
}
