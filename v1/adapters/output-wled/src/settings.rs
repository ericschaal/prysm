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

/// LEDs per edge, as seen from the front of the screen. An omitted edge has none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct LedCounts {
    #[cfg_attr(feature = "schema", schema(minimum = 0, maximum = 65535))]
    pub top: u16,
    #[cfg_attr(feature = "schema", schema(minimum = 0, maximum = 65535))]
    pub right: u16,
    #[cfg_attr(feature = "schema", schema(minimum = 0, maximum = 65535))]
    pub bottom: u16,
    #[cfg_attr(feature = "schema", schema(minimum = 0, maximum = 65535))]
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
}

impl WledSettings {
    /// # Errors
    /// An address that is not `host[:port]` with a unicast IP literal or a valid
    /// host name, or a total LED count outside 1..=65535.
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.device()?;
        if !(1..=u32::from(u16::MAX)).contains(&self.leds.total()) {
            return Err(ValidationError::new(
                "leds",
                "total LED count must be between 1 and 65535",
            ));
        }
        Ok(())
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
        assert_eq!(schema["properties"]["top"]["maximum"], json!(65535));
    }
}
