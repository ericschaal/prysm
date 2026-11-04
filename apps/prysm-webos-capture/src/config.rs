//! Internal capture defaults; only startup choices cross the supervisor boundary.
use prysm_webos_protocol::InstanceUrl;
pub use prysm_webos_protocol::{Compression, Source};

#[derive(Debug)]
pub struct Config {
    pub instance_url: InstanceUrl,
    pub source: Source,
    pub compression: Compression,
    pub capture: CaptureOptions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureOptions {
    pub width: u32,
    pub height: u32,
    pub fps: u8,
}

impl Config {
    pub fn new(instance_url: InstanceUrl) -> Self {
        Self {
            instance_url,
            source: Source::Both,
            compression: Compression::None,
            capture: CaptureOptions::default(),
        }
    }
}

impl From<prysm_webos_protocol::WorkerConfig> for Config {
    fn from(settings: prysm_webos_protocol::WorkerConfig) -> Self {
        Self {
            source: settings.source,
            compression: settings.compression,
            ..Self::new(settings.instance_url)
        }
    }
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            width: 320,
            height: 180,
            fps: 30,
        }
    }
}
