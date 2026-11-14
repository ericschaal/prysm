//! Saved color-correction profiles, in the controls a user edits.
use crate::{Section, ValidationError};
use prysm_core::ColorCorrection;
use serde::{Deserialize, Serialize};

/// Most profiles one settings file may hold.
pub const MAX_PROFILES: usize = 32;
/// Longest profile id, in bytes (ids are ASCII).
pub const MAX_PROFILE_ID_LEN: usize = 64;
/// Longest profile name, in characters.
pub const MAX_PROFILE_NAME_CHARS: usize = 64;

/// A named correction the user can make active, such as one per console.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorProfile {
    /// Stable reference for `active_profile`: 1–64 ASCII letters, digits,
    /// `-` or `_` (a UUID fits). Unique within the list.
    pub id: String,
    /// Shown to the user: 1–64 characters, no surrounding whitespace or
    /// control characters.
    pub name: String,
    #[serde(default)]
    pub correction: CorrectionControls,
}

/// Correction controls, each -100 to 100; all zero is no correction.
/// Converted to [`ColorCorrection`] for the processor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CorrectionControls {
    /// Positive warms (attenuates blue), negative cools (attenuates red).
    pub warmth: i16,
    /// Positive adds magenta (attenuates green), negative adds green.
    pub tint: i16,
    /// Positive lightens midtones, negative darkens them.
    pub midtones: i16,
    /// Chroma: positive saturates, negative desaturates.
    pub intensity: i16,
    pub hue: HueControls,
}

/// Hue shift per wheel anchor, each -100 to 100. Positive turns toward the
/// next anchor in red, yellow, green, cyan, blue, magenta order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HueControls {
    pub red: i16,
    pub yellow: i16,
    pub green: i16,
    pub cyan: i16,
    pub blue: i16,
    pub magenta: i16,
}

impl HueControls {
    /// Wheel order, as [`ColorCorrection::with_chroma`] takes them.
    #[must_use]
    pub const fn to_array(self) -> [i16; 6] {
        [
            self.red,
            self.yellow,
            self.green,
            self.cyan,
            self.blue,
            self.magenta,
        ]
    }

    #[must_use]
    pub const fn from_array([red, yellow, green, cyan, blue, magenta]: [i16; 6]) -> Self {
        Self {
            red,
            yellow,
            green,
            cyan,
            blue,
            magenta,
        }
    }

    fn named(self) -> [(&'static str, i16); 6] {
        let [red, yellow, green, cyan, blue, magenta] = self.to_array();
        [
            ("red", red),
            ("yellow", yellow),
            ("green", green),
            ("cyan", cyan),
            ("blue", blue),
            ("magenta", magenta),
        ]
    }
}

impl CorrectionControls {
    /// # Errors
    /// The first control outside -100..=100, named under `field`.
    pub fn validate(&self, field: &str) -> Result<(), ValidationError> {
        let plain = [
            ("warmth", self.warmth),
            ("tint", self.tint),
            ("midtones", self.midtones),
            ("intensity", self.intensity),
        ]
        .map(|(name, value)| (format!("{field}.{name}"), value));
        let hue = self
            .hue
            .named()
            .map(|(name, value)| (format!("{field}.hue.{name}"), value));
        for (name, value) in plain.into_iter().chain(hue) {
            if !(-100..=100).contains(&value) {
                return Err(ValidationError::new(name, "must be -100 to 100"));
            }
        }
        Ok(())
    }

    /// The processor's correction for these controls; `None` when a control
    /// is outside -100..=100, which [`Processing::validate`] reports with
    /// its settings path.
    ///
    /// [`Processing::validate`]: crate::Processing::validate
    #[must_use]
    pub fn to_core(&self) -> Option<ColorCorrection> {
        ColorCorrection::new(self.warmth, self.tint, self.midtones)
            .and_then(|c| c.with_chroma(self.intensity, self.hue.to_array()))
            .ok()
    }

    /// Recover the controls a [`ColorCorrection`] was built from. Exact for
    /// every correction [`CorrectionControls::to_core`] returns.
    #[must_use]
    pub fn from_core(correction: &ColorCorrection) -> Self {
        // to_core: gains = 2^(w/200), 2^(-t/200), 2^(-w/200), divided by their
        // maximum (which cancels in ratios); tone = 2^(-m/100).
        let [red, green, blue] = correction.gains();
        let warmth = 100.0 * (red / blue).log2();
        let tint = warmth - 200.0 * (green / blue).log2();
        let midtones = -100.0 * correction.tone().log2();
        Self {
            warmth: control(warmth),
            tint: control(tint),
            midtones: control(midtones),
            intensity: correction.intensity(),
            hue: HueControls::from_array(correction.hue()),
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "rounded and clamped to the control range first"
)]
fn control(value: f32) -> i16 {
    value.round().clamp(-100.0, 100.0) as i16
}

/// Check a profile list and its active reference. Fields are named under
/// `section`, for example `processing.profiles[2].name`.
pub(crate) fn validate(
    section: Section,
    profiles: &[ColorProfile],
    active: Option<&str>,
) -> Result<(), ValidationError> {
    if profiles.len() > MAX_PROFILES {
        return Err(ValidationError::new(
            format!("{section}.profiles"),
            format!("at most {MAX_PROFILES} profiles are allowed"),
        ));
    }
    for (index, profile) in profiles.iter().enumerate() {
        let field = |name: &str| format!("{section}.profiles[{index}].{name}");
        if !valid_id(&profile.id) {
            return Err(ValidationError::new(
                field("id"),
                format!("must be 1-{MAX_PROFILE_ID_LEN} ASCII letters, digits, '-' or '_'"),
            ));
        }
        if let Some(first) = profiles[..index].iter().position(|p| p.id == profile.id) {
            return Err(ValidationError::new(
                field("id"),
                format!("duplicates profiles[{first}].id `{}`", profile.id),
            ));
        }
        if !valid_name(&profile.name) {
            return Err(ValidationError::new(
                field("name"),
                format!(
                    "must be 1-{MAX_PROFILE_NAME_CHARS} characters without surrounding \
                     whitespace or control characters"
                ),
            ));
        }
        profile.correction.validate(&field("correction"))?;
    }
    if let Some(active) = active
        && !profiles.iter().any(|p| p.id == active)
    {
        return Err(ValidationError::new(
            format!("{section}.active_profile"),
            format!("no profile has id `{active}`"),
        ));
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    (1..=MAX_PROFILE_ID_LEN).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name == name.trim()
        && name.chars().count() <= MAX_PROFILE_NAME_CHARS
        && !name.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, name: &str) -> ColorProfile {
        ColorProfile {
            id: id.into(),
            name: name.into(),
            correction: CorrectionControls::default(),
        }
    }

    fn field(profiles: &[ColorProfile], active: Option<&str>) -> String {
        validate(Section::Processing, profiles, active)
            .unwrap_err()
            .field
    }

    #[test]
    fn controls_map_to_core_and_back_exactly() {
        assert_eq!(
            CorrectionControls::default().to_core().unwrap(),
            ColorCorrection::default()
        );
        // Every white-balance pair, across the midtone range.
        for warmth in -100..=100 {
            for tint in -100..=100 {
                for midtones in [-100, -37, 0, 1, 64, 100] {
                    let controls = CorrectionControls {
                        warmth,
                        tint,
                        midtones,
                        ..CorrectionControls::default()
                    };
                    let core = controls.to_core().unwrap();
                    assert_eq!(
                        core,
                        ColorCorrection::new(warmth, tint, midtones).unwrap(),
                        "{controls:?}"
                    );
                    assert_eq!(CorrectionControls::from_core(&core), controls);
                }
            }
        }
        for value in -100..=100 {
            let controls = CorrectionControls {
                midtones: value,
                intensity: value,
                hue: HueControls::from_array([value, -value, 0, value / 2, 100, -100]),
                ..CorrectionControls::default()
            };
            let core = controls.to_core().unwrap();
            assert_eq!(core.intensity(), value);
            assert_eq!(core.hue(), controls.hue.to_array());
            assert_eq!(CorrectionControls::from_core(&core), controls);
        }
    }

    #[test]
    fn controls_outside_their_range_are_named() {
        let bad = CorrectionControls {
            tint: -101,
            ..CorrectionControls::default()
        };
        assert_eq!(bad.validate("c").unwrap_err().field, "c.tint");
        assert_eq!(bad.to_core(), None);
        let bad = CorrectionControls {
            hue: HueControls {
                cyan: 101,
                ..HueControls::default()
            },
            ..CorrectionControls::default()
        };
        assert_eq!(bad.validate("c").unwrap_err().field, "c.hue.cyan");
        assert_eq!(bad.to_core(), None);
    }

    #[test]
    fn profile_lists_are_bounded_unique_named_and_referenced() {
        validate(Section::Processing, &[], None).unwrap();
        let two = [profile("ps5", "PS5"), profile("tv-apps", "TV apps")];
        validate(Section::Processing, &two, Some("tv-apps")).unwrap();
        let uuid = profile("0b6f2c1e-1d2a-4c3b-9f4e-5a6b7c8d9e0f", "x");
        validate(Section::Processing, &[uuid], None).unwrap();

        assert_eq!(
            field(&two, Some("gone")),
            "processing.active_profile",
            "unknown active id"
        );
        assert_eq!(
            field(&[profile("a", "A"), profile("a", "B")], None),
            "processing.profiles[1].id"
        );
        for id in ["", "has space", "ünï", &"x".repeat(65)] {
            assert_eq!(
                field(&[profile(id, "A")], None),
                "processing.profiles[0].id"
            );
        }
        for name in ["", " ", " lead", "trail ", "line\nbreak", &"é".repeat(65)] {
            assert_eq!(
                field(&[profile("a", name)], None),
                "processing.profiles[0].name",
                "{name:?}"
            );
        }
        validate(Section::Processing, &[profile("a", &"é".repeat(64))], None).unwrap();

        let mut out_of_range = profile("a", "A");
        out_of_range.correction.hue.magenta = -101;
        assert_eq!(
            field(&[profile("b", "B"), out_of_range], None),
            "processing.profiles[1].correction.hue.magenta"
        );

        let many: Vec<_> = (0..=MAX_PROFILES)
            .map(|i| profile(&i.to_string(), "P"))
            .collect();
        validate(Section::Processing, &many[..MAX_PROFILES], None).unwrap();
        assert_eq!(field(&many, None), "processing.profiles");
    }
}
