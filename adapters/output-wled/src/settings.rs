use crate::address::DeviceAddress;
use prysm_ports::ValidationError;
use serde::{Deserialize, Serialize};

/// Where the device is and how many LEDs each screen edge has:
///
/// ```json
/// { "kind": "wled", "address": "wled.local",
///   "leds": { "top": 96, "right": 54, "bottom": 96, "left": 54 } }
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
#[serde(default, deny_unknown_fields)]
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
    use serde::de::DeserializeOwned;
    use serde_json::{Map, Value, json};
    use std::collections::BTreeSet;
    use utoipa::ToSchema;

    /// The schema lists exactly the serialized fields, requires exactly those
    /// serde cannot default, and rejects unknown fields as serde does.
    fn same_shape<T: ToSchema + Serialize + DeserializeOwned>(full: &T) -> Value {
        let schema = serde_json::to_value(T::schema()).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        let required: Vec<String> =
            serde_json::from_value(schema["required"].clone()).unwrap_or_default();
        let Value::Object(fields) = serde_json::to_value(full).unwrap() else {
            panic!("settings serialize as an object");
        };
        let keys = |map: &Map<String, Value>| map.keys().cloned().collect::<BTreeSet<_>>();
        assert_eq!(keys(&fields), keys(properties));
        let minimal: Map<String, Value> = fields
            .iter()
            .filter(|(key, _)| required.contains(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        serde_json::from_value::<T>(minimal.clone().into()).unwrap();
        for key in &required {
            let mut missing = minimal.clone();
            missing.remove(key);
            assert!(
                serde_json::from_value::<T>(missing.into()).is_err(),
                "{key}"
            );
        }
        assert_eq!(schema["additionalProperties"], json!(false));
        let mut unknown = fields;
        unknown.insert("unknown".into(), json!(1));
        assert!(serde_json::from_value::<T>(unknown.into()).is_err());
        schema
    }

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
        };
        let schema = same_shape(&settings);
        assert_eq!(schema["required"], json!(["address", "leds"]));
        let address = &schema["properties"]["address"];
        assert_eq!(address["type"], json!("string"));
        assert_eq!(address["minLength"], json!(1));
        for example in address["examples"].as_array().unwrap() {
            let settings = WledSettings {
                address: example.as_str().unwrap().into(),
                leds,
            };
            assert_eq!(settings.validate(), Ok(()), "{example}");
        }
        let schema = same_shape(&leds);
        assert_eq!(schema.get("required"), None);
        assert_eq!(schema["properties"]["top"]["maximum"], json!(16384));
    }
}
