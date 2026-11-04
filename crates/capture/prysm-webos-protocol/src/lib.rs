//! Bounded webOS frames: full-range RGB, or limited-range BT.709 YUV (including H.264).
//! Also owns the shared capture settings and private supervisor/worker IPC types.
mod control;
mod frame;
#[cfg(feature = "h264")]
pub mod h264;
pub mod settings;
pub mod worker;

pub use control::{ControlMessage, HELLO, MAX_CONTROL_LEN};
pub use frame::{Codec, HEADER_LEN, Header, MAGIC, PixelFormat, compress, decompress};
pub use settings::{Compression, InstanceUrl, Source, WORKER_CONFIG_ENV, WorkerConfig};

pub const CAPTURE_PATH: &str = "/api/capture";

/// An idle receiver renews the wait until it can issue capture settings.
pub const IDLE_RENEWAL: std::time::Duration = std::time::Duration::from_secs(1);
pub const CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// One ASCII digit also forms the last byte of the four-byte frame magic.
pub const VERSION: u32 = {
    let text = include_bytes!("../wire-version.txt");
    assert!(text.len() == 2 && text[0].is_ascii_digit() && text[1] == b'\n');
    (text[0] - b'0') as u32
};
