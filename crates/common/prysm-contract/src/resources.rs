//! Validated capture inputs, resolutions, and renderer outputs.
use serde::{Deserialize, Serialize};
use std::{num::NonZeroU32, path::Path};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "[u32; 2]", into = "[u32; 2]")]
pub struct CaptureResolution([u32; 2]);
impl utoipa::PartialSchema for CaptureResolution {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{ArrayBuilder, ObjectBuilder, Type};
        ArrayBuilder::new()
            .items(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(1.0))
                    .maximum(Some(f64::from(u32::MAX))),
            )
            .min_items(Some(2))
            .max_items(Some(2))
            .into()
    }
}
impl ToSchema for CaptureResolution {}
impl CaptureResolution {
    pub fn new(width: u32, height: u32) -> Result<Self, &'static str> {
        prysm_core::CaptureResolution::new(width, height)?;
        Ok(Self([width, height]))
    }
    #[must_use]
    pub const fn dimensions(self) -> [u32; 2] {
        self.0
    }
    #[must_use]
    pub fn to_core(self) -> prysm_core::CaptureResolution {
        prysm_core::CaptureResolution::new(self.0[0], self.0[1]).expect("validated resolution")
    }
}
impl Default for CaptureResolution {
    fn default() -> Self {
        Self(prysm_core::CaptureResolution::default().dimensions())
    }
}
impl TryFrom<[u32; 2]> for CaptureResolution {
    type Error = &'static str;
    fn try_from([width, height]: [u32; 2]) -> Result<Self, Self::Error> {
        Self::new(width, height)
    }
}
impl From<CaptureResolution> for [u32; 2] {
    fn from(value: CaptureResolution) -> Self {
        value.0
    }
}

macro_rules! nonempty_string {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl TryFrom<String> for $name {
            type Error = &'static str;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                if value.trim().is_empty() {
                    Err(concat!(stringify!($name), " must not be empty"))
                } else {
                    Ok(Self(value))
                }
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl $name {
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}
nonempty_string!(DeviceId);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
pub struct WledAddress(String);
impl TryFrom<String> for WledAddress {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parsed = url::Url::parse(&format!("udp://{value}"))
            .map_err(|_| "DDP address must be host:port")?;
        if parsed.host_str().is_none()
            || parsed.port().is_none_or(|port| port == 0)
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || !parsed.path().is_empty()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err("DDP address must be host:port");
        }
        Ok(Self(value))
    }
}
impl From<WledAddress> for String {
    fn from(value: WledAddress) -> Self {
        value.0
    }
}
impl WledAddress {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
nonempty_string!(VideoPath);
impl VideoPath {
    #[must_use]
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawLedCounts", into = "RawLedCounts")]
pub struct LedCounts([usize; 4]);
impl utoipa::PartialSchema for LedCounts {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        RawLedCounts::schema()
    }
}
impl ToSchema for LedCounts {}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct RawLedCounts {
    top: u16,
    right: u16,
    bottom: u16,
    left: u16,
}
impl TryFrom<RawLedCounts> for LedCounts {
    type Error = &'static str;
    fn try_from(value: RawLedCounts) -> Result<Self, Self::Error> {
        let counts = [value.top, value.right, value.bottom, value.left].map(usize::from);
        if !(1..=usize::from(u16::MAX)).contains(&counts.iter().sum()) {
            return Err("total LED count must be between 1 and 65535");
        }
        Ok(Self(counts))
    }
}
impl From<LedCounts> for RawLedCounts {
    fn from(value: LedCounts) -> Self {
        let [top, right, bottom, left] = value
            .0
            .map(|value| u16::try_from(value).expect("validated LED count"));
        Self {
            top,
            right,
            bottom,
            left,
        }
    }
}
impl LedCounts {
    #[must_use]
    pub const fn counts(self) -> [usize; 4] {
        self.0
    }
}
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Input {
    Device {
        device: DeviceId,
        #[schema(value_type = u32, minimum = 1)]
        refresh_rate_hz: NonZeroU32,
    },
    File {
        path: VideoPath,
        #[schema(value_type = u32, minimum = 1)]
        refresh_rate_hz: NonZeroU32,
    },
    Webos {
        #[schema(value_type = String)]
        listen: std::net::SocketAddr,
        #[schema(value_type = u32, minimum = 1)]
        refresh_rate_hz: NonZeroU32,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Output {
    Wled {
        address: WledAddress,
        leds: LedCounts,
    },
    Egui {},
}

impl Output {
    #[must_use]
    pub const fn is_egui(&self) -> bool {
        matches!(self, Self::Egui { .. })
    }
}
