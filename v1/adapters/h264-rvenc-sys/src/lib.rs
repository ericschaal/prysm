//! The LG webOS TV's hardware H.264 encoder (`rvenc`, a GStreamer element)
//! fed with NV12 in DMA-BUFs from `/dev/dma_heap/dvr`. Ported from the
//! qualified `apps/prysm-webos-capture/src/encoder/gst.rs`; the contract is
//! `docs/developers/hardware-h264.md`.
//!
//! [`Pipeline`] is the one safe type: `appsrc ! rvenc ! appsink`, one picture
//! in flight. Bitstream validation and SPS repair belong to the caller
//! (`prysm-h264`); this crate only moves pixels in and bytes out.
//!
//! - Two persistent DMA-BUFs, each mapped for its slot's lifetime, with a
//!   192-byte pitch. Each upload is bracketed by `DMA_BUF_IOCTL_SYNC`
//!   start/end, and a slot is reused only after GStreamer released it
//!   (per-frame allocation corrupted pictures on the TV).
//! - `rvenc` output goes straight to `appsink` (no `h264parse`, which added a
//!   frame of latency on the TV). Output wait 1 s, EOS wait 500 ms on drop.
//!
//! Only 32-bit ARM Linux (the TV) builds the pipeline and links GStreamer and
//! GLib; on every other target [`Pipeline::new`] fails with
//! [`Error::Unavailable`].
#[cfg(all(target_os = "linux", target_arch = "arm"))]
mod gst;
mod layout;

#[cfg(all(target_os = "linux", target_arch = "arm"))]
pub use gst::Pipeline;
pub use layout::{MAX_OUTPUT_BYTES, PITCH, copy_nv12, pitch};

/// Why the hardware encoder cannot start or encode.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Not a webOS TV build.
    #[error("hardware H.264 is only built for the webOS TV (32-bit ARM Linux)")]
    Unavailable,
    #[error("invalid hardware encoder configuration: {0}")]
    Config(&'static str),
    /// GStreamer could not initialise, build or run the pipeline (no `rvenc`
    /// element, a state change refused, a buffer rejected).
    #[error("GStreamer: {0}")]
    Gstreamer(String),
    #[error("{context}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("the hardware encoder produced no picture within 1 s")]
    Timeout,
    #[error("the hardware encoder still holds the input buffer")]
    Busy,
    #[error("invalid hardware output: {0}")]
    Output(&'static str),
}

#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
mod stub {
    use super::Error;

    /// Stands in for the TV pipeline on other targets: never constructed.
    #[derive(Debug)]
    pub struct Pipeline(());

    impl Pipeline {
        /// # Errors
        /// Always [`Error::Unavailable`] off the TV.
        pub fn new(
            _width: u32,
            _height: u32,
            _stride: usize,
            _fps: u32,
            _bitrate: u32,
        ) -> Result<Self, Error> {
            Err(Error::Unavailable)
        }

        /// # Errors
        /// Always [`Error::Unavailable`] off the TV.
        #[allow(clippy::unused_self, reason = "the TV pipeline's signature")]
        pub fn encode(&mut self, _: &[u8], _: usize, _: &mut Vec<u8>) -> Result<(), Error> {
            Err(Error::Unavailable)
        }
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
pub use stub::Pipeline;
