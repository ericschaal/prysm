//! Identifiers shared by saved configuration and controller operations.
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, pattern = "^(0|[1-9][0-9]*)$")]
pub struct ConfigRevision(u64);
impl ConfigRevision {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
    pub fn next(self) -> Result<Self, &'static str> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or("configuration revision exhausted")
    }
}
impl TryFrom<String> for ConfigRevision {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let number = value
            .parse::<u64>()
            .map_err(|_| "invalid configuration revision")?;
        if number.to_string() != value {
            return Err("configuration revision must be a canonical decimal string");
        }
        Ok(Self(number))
    }
}
impl From<ConfigRevision> for String {
    fn from(value: ConfigRevision) -> Self {
        value.0.to_string()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct OperationId(Uuid);
impl OperationId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Default for OperationId {
    fn default() -> Self {
        Self::new()
    }
}
