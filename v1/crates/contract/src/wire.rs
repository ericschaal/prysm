//! Serde adapters for reused core representations.
pub mod decimal {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        let value = text.parse::<u64>().map_err(D::Error::custom)?;
        if value.to_string() != text {
            return Err(D::Error::custom("expected canonical decimal u64"));
        }
        Ok(value)
    }
}

pub mod nonzero_decimal {
    use serde::{Deserializer, Serializer, de::Error};
    use std::num::NonZeroU64;
    pub fn serialize<S: Serializer>(value: &NonZeroU64, serializer: S) -> Result<S::Ok, S::Error> {
        super::decimal::serialize(&value.get(), serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<NonZeroU64, D::Error> {
        NonZeroU64::new(super::decimal::deserialize(deserializer)?)
            .ok_or_else(|| D::Error::custom("expected nonzero id"))
    }
}
pub mod resolution {
    use prysm_core::CaptureResolution;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

    pub fn serialize<S: Serializer>(
        value: &CaptureResolution,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.dimensions().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<CaptureResolution, D::Error> {
        let [width, height] = <[u32; 2]>::deserialize(deserializer)?;
        CaptureResolution::new(width, height).map_err(D::Error::custom)
    }
}
