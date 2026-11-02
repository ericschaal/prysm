//! Internal capture defaults; only startup choices cross the supervisor boundary.
use prysm_webos_protocol::ReceiverAddress;
pub use prysm_webos_protocol::{Compression, Source};

#[derive(Debug)]
pub struct Config {
    pub receiver: ReceiverAddress,
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
    pub fn new(receiver: ReceiverAddress) -> Self {
        Self {
            receiver,
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
            ..Self::new(settings.receiver)
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
