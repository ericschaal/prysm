use crate::address::DeviceAddress;
use prysm_ports::ValidationError;
use serde::{Deserialize, Serialize};

/// Where the device is, how many LEDs each screen edge has, and whether
/// dim colours are dithered (on unless `false`):
///
/// ```json
/// { "kind": "wled", "address": "wled.local",
///   "leds": { "top": 96, "right": 54, "bottom": 96, "left": 54 },
///   "dither": true }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct WledSettings {
    /// Host name or unicast IP address of the device, with an optional HTTP
    /// port (default 80): `wled.local`, `192.168.1.50`, `wled.local:8080` or
    /// `[fe80::1]:8080`. No scheme or path; WLED serves plain HTTP. Names are
    /// resolved each time the output opens. Colours always go to UDP port 4048.
    #[cfg_attr(
        feature = "schema",
        schema(
            min_length = 1,
            examples("wled.local", "192.168.1.50", "wled.local:8080", "[fe80::1]:8080")
        )
    )]
    pub address: String,
    pub leds: LedCounts,
    /// Mix neighbouring LEDs between the few levels WLED shows for dim
    /// colours, so they fade evenly and keep their hue. False: each LED
    /// takes the nearest byte, for a strip on which dark scenes twinkle.
    #[serde(default = "dither_by_default")]
    #[cfg_attr(feature = "schema", schema(default = true))]
    pub dither: bool,
}

fn dither_by_default() -> bool {
    true
}

/// The most LEDs a WLED device drives: `MAX_LEDS` of WLED's largest default
/// builds (ESP32, S3, P4; smaller chips stop earlier). DDP itself and the
/// renderer take more, but no stock WLED shows them, so a larger count is a
/// typo rather than a strip. It bounds each edge and the total alike: one
/// edge may hold every LED.
pub const MAX_LEDS: u16 = 16_384;

/// LEDs per edge, as seen from the front of the screen. An omitted edge has
/// none. Together 1..=[`MAX_LEDS`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(remote = "Self", default, deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct LedCounts {
    #[cfg_attr(feature = "schema", schema(minimum = 0, maximum = 16384))]
    pub top: u16,
    #[cfg_attr(feature = "schema", schema(minimum = 0, maximum = 16384))]
    pub right: u16,
    #[cfg_attr(feature = "schema", schema(minimum = 0, maximum = 16384))]
    pub bottom: u16,
    #[cfg_attr(feature = "schema", schema(minimum = 0, maximum = 16384))]
    pub left: u16,
}

prysm_ports::map_only!(LedCounts, "LED counts per edge as an object");

impl LedCounts {
    #[must_use]
    pub fn total(self) -> u32 {
        [self.top, self.right, self.bottom, self.left]
            .into_iter()
            .map(u32::from)
            .sum()
    }

    /// Top, right, bottom, left: the renderer's order.
    pub(crate) fn edges(self) -> [usize; 4] {
        [self.top, self.right, self.bottom, self.left].map(usize::from)
    }

    /// # Errors
    /// An edge over [`MAX_LEDS`], at `leds.<edge>`; otherwise a total
    /// outside 1..=[`MAX_LEDS`], at `leds`.
    fn validate(self) -> Result<(), ValidationError> {
        for (edge, count) in [
            ("leds.top", self.top),
            ("leds.right", self.right),
            ("leds.bottom", self.bottom),
            ("leds.left", self.left),
        ] {
            if count > MAX_LEDS {
                return Err(ValidationError::new(
                    edge,
                    format!("an edge can have at most {MAX_LEDS} LEDs, the most WLED drives"),
                ));
            }
        }
        if !(1..=u32::from(MAX_LEDS)).contains(&self.total()) {
            return Err(ValidationError::new(
                "leds",
                format!("the LEDs must total 1 to {MAX_LEDS}, the most WLED drives"),
            ));
        }
        Ok(())
    }
}

impl WledSettings {
    /// # Errors
    /// An address that is not `host[:port]` with a unicast IP literal or a valid
    /// host name, or LED counts outside their limits ([`MAX_LEDS`]).
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.device()?;
        self.leds.validate()
    }

    pub(crate) fn device(&self) -> Result<DeviceAddress, ValidationError> {
        DeviceAddress::parse(&self.address)
            .map_err(|reason| ValidationError::new("address", reason))
    }
}

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    use prysm_testkit::assert_schema_matches_serde;
    use serde_json::json;

    #[test]
    fn schema_matches_serde_shape() {
        let leds = LedCounts {
            top: 96,
            right: 54,
            bottom: 96,
            left: 54,
        };
        let settings = WledSettings {
            address: "wled.local:8080".into(),
            leds,
            dither: false,
        };
        let schema = assert_schema_matches_serde(&settings);
        assert_eq!(schema["required"], json!(["address", "leds"]));
        assert_eq!(schema["properties"]["dither"]["default"], json!(true));
        let omitted: WledSettings =
            serde_json::from_value(json!({"address": "wled.local", "leds": {"top": 1}})).unwrap();
        assert!(omitted.dither, "dithering is on unless turned off");
        let address = &schema["properties"]["address"];
        assert_eq!(address["type"], json!("string"));
        assert_eq!(address["minLength"], json!(1));
        for example in address["examples"].as_array().unwrap() {
            let settings = WledSettings {
                address: example.as_str().unwrap().into(),
                leds,
                dither: true,
            };
            assert_eq!(settings.validate(), Ok(()), "{example}");
        }
        let schema = assert_schema_matches_serde(&leds);
        assert_eq!(schema.get("required"), None);
        assert_eq!(schema["properties"]["top"]["maximum"], json!(16384));
    }
}
