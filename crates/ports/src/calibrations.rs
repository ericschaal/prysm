//! Saved calibrations, in the controls a user edits.
use crate::{Section, ValidationError};
use prysm_core::{ColorCorrection, PrimaryAdjustment};
use serde::{Deserialize, Serialize};

/// Most calibrations one settings file may hold.
pub const MAX_CALIBRATIONS: usize = 32;
/// Longest calibration id, in bytes (ids are ASCII).
pub const MAX_CALIBRATION_ID_LEN: usize = 64;
/// Longest calibration name, in characters.
pub const MAX_CALIBRATION_NAME_CHARS: usize = 64;

/// A named correction the user can make active, such as one per console.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    /// Stable reference for `active_calibration`: 1–64 ASCII letters, digits,
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
    pub red: PrimaryControls,
    pub green: PrimaryControls,
    pub blue: PrimaryControls,
}

/// One picture primary's controls, each -100 to 100. White stays as the
/// white balance leaves it whatever they are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrimaryControls {
    /// Positive turns the colour toward the next primary in red, green, blue
    /// order (red more orange, green more turquoise, blue more purple),
    /// negative toward the previous one (more pink, yellow, turquoise).
    pub hue: i16,
    /// Negative makes the colour softer, positive stronger.
    pub strength: i16,
}

impl From<PrimaryControls> for PrimaryAdjustment {
    fn from(PrimaryControls { hue, strength }: PrimaryControls) -> Self {
        Self { hue, strength }
    }
}

impl CorrectionControls {
    /// # Errors
    /// The first control outside -100..=100, named under `field`.
    pub fn validate(&self, field: &str) -> Result<(), ValidationError> {
        let check = |name: &str, value: i16| {
            if (-100..=100).contains(&value) {
                Ok(())
            } else {
                Err(ValidationError::new(
                    format!("{field}.{name}"),
                    "must be -100 to 100",
                ))
            }
        };
        check("warmth", self.warmth)?;
        check("tint", self.tint)?;
        check("midtones", self.midtones)?;
        for (name, primary) in [
            ("red", self.red),
            ("green", self.green),
            ("blue", self.blue),
        ] {
            check(&format!("{name}.hue"), primary.hue)?;
            check(&format!("{name}.strength"), primary.strength)?;
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
            .and_then(|c| c.with_primaries([self.red, self.green, self.blue].map(Into::into)))
            .ok()
    }
}

/// Check a calibration list and its active reference. Fields are named under
/// `section`, for example `processing.calibrations[2].name`.
pub(crate) fn validate(
    section: Section,
    calibrations: &[Calibration],
    active: Option<&str>,
) -> Result<(), ValidationError> {
    if calibrations.len() > MAX_CALIBRATIONS {
        return Err(ValidationError::new(
            format!("{section}.calibrations"),
            format!("at most {MAX_CALIBRATIONS} calibrations are allowed"),
        ));
    }
    for (index, calibration) in calibrations.iter().enumerate() {
        let field = |name: &str| format!("{section}.calibrations[{index}].{name}");
        if !valid_id(&calibration.id) {
            return Err(ValidationError::new(
                field("id"),
                format!("must be 1-{MAX_CALIBRATION_ID_LEN} ASCII letters, digits, '-' or '_'"),
            ));
        }
        if let Some(first) = calibrations[..index]
            .iter()
            .position(|c| c.id == calibration.id)
        {
            return Err(ValidationError::new(
                field("id"),
                format!("duplicates calibrations[{first}].id `{}`", calibration.id),
            ));
        }
        if !valid_name(&calibration.name) {
            return Err(ValidationError::new(
                field("name"),
                format!(
                    "must be 1-{MAX_CALIBRATION_NAME_CHARS} characters without surrounding \
                     whitespace or control characters"
                ),
            ));
        }
        calibration.correction.validate(&field("correction"))?;
    }
    if let Some(active) = active
        && !calibrations.iter().any(|c| c.id == active)
    {
        return Err(ValidationError::new(
            format!("{section}.active_calibration"),
            format!("no calibration has id `{active}`"),
        ));
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    (1..=MAX_CALIBRATION_ID_LEN).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name == name.trim()
        && name.chars().count() <= MAX_CALIBRATION_NAME_CHARS
        && !name.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calibration(id: &str, name: &str) -> Calibration {
        Calibration {
            id: id.into(),
            name: name.into(),
            correction: CorrectionControls::default(),
        }
    }

    fn field(calibrations: &[Calibration], active: Option<&str>) -> String {
        validate(Section::Processing, calibrations, active)
            .unwrap_err()
            .field
    }

    #[test]
    fn controls_map_to_the_core_correction() {
        assert_eq!(
            CorrectionControls::default().to_core().unwrap(),
            ColorCorrection::default()
        );
        let red = PrimaryControls {
            hue: 20,
            strength: -35,
        };
        let blue = PrimaryControls {
            hue: -100,
            strength: 100,
        };
        let controls = CorrectionControls {
            warmth: 12,
            tint: -4,
            midtones: 6,
            red,
            blue,
            ..CorrectionControls::default()
        };
        let expected = ColorCorrection::new(12, -4, 6)
            .unwrap()
            .with_primaries([red.into(), PrimaryAdjustment::default(), blue.into()])
            .unwrap();
        assert_eq!(controls.to_core().unwrap(), expected);
    }

    #[test]
    fn controls_outside_their_range_are_named() {
        let bad = CorrectionControls {
            tint: -101,
            ..CorrectionControls::default()
        };
        assert_eq!(bad.validate("c").unwrap_err().field, "c.tint");
        assert_eq!(bad.to_core(), None);
        for (primary, field) in [
            (
                PrimaryControls {
                    hue: 101,
                    strength: 0,
                },
                "c.green.hue",
            ),
            (
                PrimaryControls {
                    hue: 0,
                    strength: -101,
                },
                "c.green.strength",
            ),
        ] {
            let bad = CorrectionControls {
                green: primary,
                ..CorrectionControls::default()
            };
            assert_eq!(bad.validate("c").unwrap_err().field, field);
            assert_eq!(bad.to_core(), None);
        }
    }

    #[test]
    fn controls_are_read_and_written_with_every_field() {
        let json = serde_json::json!({
            "warmth": 5,
            "red": { "hue": 10 },
            "blue": { "strength": -20 }
        });
        let controls: CorrectionControls = serde_json::from_value(json).unwrap();
        assert_eq!(controls.red.hue, 10);
        assert_eq!(controls.blue.strength, -20);
        let written = serde_json::to_value(controls).unwrap();
        assert_eq!(
            written,
            serde_json::json!({
                "warmth": 5, "tint": 0, "midtones": 0,
                "red": { "hue": 10, "strength": 0 },
                "green": { "hue": 0, "strength": 0 },
                "blue": { "hue": 0, "strength": -20 }
            })
        );
        assert_eq!(
            serde_json::from_value::<CorrectionControls>(written).unwrap(),
            controls
        );
        for unknown in [
            serde_json::json!({ "intensity": 0 }),
            serde_json::json!({ "hue": { "red": 0 } }),
            serde_json::json!({ "red": { "saturation": 0 } }),
        ] {
            assert!(serde_json::from_value::<CorrectionControls>(unknown).is_err());
        }
    }

    #[test]
    fn calibration_lists_are_bounded_unique_named_and_referenced() {
        validate(Section::Processing, &[], None).unwrap();
        let two = [calibration("ps5", "PS5"), calibration("tv-apps", "TV apps")];
        validate(Section::Processing, &two, Some("tv-apps")).unwrap();
        let uuid = calibration("0b6f2c1e-1d2a-4c3b-9f4e-5a6b7c8d9e0f", "x");
        validate(Section::Processing, &[uuid], None).unwrap();

        assert_eq!(
            field(&two, Some("gone")),
            "processing.active_calibration",
            "unknown active id"
        );
        assert_eq!(
            field(&[calibration("a", "A"), calibration("a", "B")], None),
            "processing.calibrations[1].id"
        );
        for id in ["", "has space", "ünï", &"x".repeat(65)] {
            assert_eq!(
                field(&[calibration(id, "A")], None),
                "processing.calibrations[0].id"
            );
        }
        for name in ["", " ", " lead", "trail ", "line\nbreak", &"é".repeat(65)] {
            assert_eq!(
                field(&[calibration("a", name)], None),
                "processing.calibrations[0].name",
                "{name:?}"
            );
        }
        validate(
            Section::Processing,
            &[calibration("a", &"é".repeat(64))],
            None,
        )
        .unwrap();

        let mut out_of_range = calibration("a", "A");
        out_of_range.correction.red.strength = -101;
        assert_eq!(
            field(&[calibration("b", "B"), out_of_range], None),
            "processing.calibrations[1].correction.red.strength"
        );

        let many: Vec<_> = (0..=MAX_CALIBRATIONS)
            .map(|i| calibration(&i.to_string(), "C"))
            .collect();
        validate(Section::Processing, &many[..MAX_CALIBRATIONS], None).unwrap();
        assert_eq!(field(&many, None), "processing.calibrations");
    }
}
