use prysm_ports::ValidationError;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Where the device is and how many LEDs each screen edge has:
///
/// ```json
/// { "kind": "wled", "address": "192.168.1.50",
///   "leds": { "top": 96, "right": 54, "bottom": 96, "left": 54 } }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct WledSettings {
    /// Unicast IPv4 or IPv6 address of the device.
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "192.168.1.50"))]
    pub address: IpAddr,
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
    /// A non-unicast address, or a total LED count outside 1..=65535.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let ip = match self.address {
            IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(self.address, IpAddr::V4),
            v4 @ IpAddr::V4(_) => v4,
        };
        let broadcast = matches!(ip, IpAddr::V4(ip) if ip.is_broadcast());
        if ip.is_unspecified() || ip.is_multicast() || broadcast {
            return Err(ValidationError::new(
                "output.address",
                "a unicast device address is required",
            ));
        }
        if !(1..=u32::from(u16::MAX)).contains(&self.leds.total()) {
            return Err(ValidationError::new(
                "output.leds",
                "total LED count must be between 1 and 65535",
            ));
        }
        Ok(())
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
            address: "192.168.1.50".parse().unwrap(),
            leds,
        };
        let schema = same_shape(&settings);
        assert_eq!(schema["required"], json!(["address", "leds"]));
        assert_eq!(schema["properties"]["address"]["type"], json!("string"));
        let schema = same_shape(&leds);
        assert_eq!(schema.get("required"), None);
        assert_eq!(schema["properties"]["top"]["maximum"], json!(65535));
    }
}
