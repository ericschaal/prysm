//! H.264 for the remote frame stream (`prysm-capture-remote`): NV12 in,
//! Annex B access units out, and back to NV12.
//!
//! - [`Encoder`] runs exactly the configured [`Backend`]: the webOS TV's
//!   hardware encoder (feature `hardware`, TV builds only) or OpenH264 in
//!   software. It never switches backend on its own; a backend that cannot
//!   start or fails is an error for the caller to report.
//! - [`Decoder`] checks each access unit (NAL types, the SPS's size, profile
//!   and colour signal) before OpenH264 sees it and writes NV12.
//!
//! Each encoder *session* starts with SPS, PPS and an IDR picture, marked
//! [`Picture::keyframe`]; a receiver decodes it with fresh decoder state, so
//! no references cross sessions. Input and output are limited-range BT.709
//! NV12, the webOS capture's native format.
//!
//! OpenH264 is Cisco's BSD-licensed codec, compiled from source; see
//! `LICENSE.openh264`.
mod decoder;
mod encoder;
#[cfg(feature = "hardware")]
mod hardware;
mod normalize;
mod software;

pub use decoder::Decoder;
pub use encoder::{Backend, Encoder, EncoderSettings, Picture};

use prysm_core::{PixelFormat, YuvRange, YuvStandardMatrix};

/// The only pixel format encoded and decoded: the webOS capture's NV12.
pub const FORMAT: PixelFormat = PixelFormat::NV12 {
    range: YuvRange::Limited,
    matrix: YuvStandardMatrix::Bt709,
};
/// The qualified target bitrate (640x280 at 30 fps on the TV).
pub const DEFAULT_BITRATE: u32 = 2_000_000;
/// Largest access unit produced or accepted.
pub const MAX_ACCESS_UNIT_BYTES: usize = 8 * 1024 * 1024;

/// Kind of a coded picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameKind {
    Idr,
    Predicted,
}

/// Why H.264 encoding or decoding failed.
#[derive(Debug, thiserror::Error)]
pub enum H264Error {
    #[error("H.264 needs limited-range BT.709 NV12 with even dimensions, not {0}")]
    Input(String),
    #[error("invalid H.264 settings: {0}")]
    Settings(&'static str),
    /// This program was built without the hardware encoder.
    #[error("this build has no hardware H.264 encoder")]
    HardwareNotBuilt,
    #[cfg(feature = "hardware")]
    #[error("hardware H.264 encoder")]
    Hardware(#[source] prysm_h264_rvenc_sys::Error),
    #[error("OpenH264")]
    OpenH264(#[source] openh264::Error),
    /// The access unit is malformed or outside what the stream allows: a
    /// received one, or the hardware encoder's before transmission.
    #[error("invalid H.264 access unit: {0}")]
    Bitstream(String),
    /// The decoder's destination is not a `width`x`height` NV12 buffer.
    #[error("the {len}-byte destination is not {width}x{height} NV12")]
    Destination { width: u32, height: u32, len: usize },
    /// The encoder returned something a session cannot use.
    #[error("the encoder misbehaved: {0}")]
    Encoder(String),
    #[error("the access unit did not produce a picture")]
    NoPicture,
}

impl From<openh264::Error> for H264Error {
    fn from(error: openh264::Error) -> Self {
        Self::OpenH264(error)
    }
}

/// `Err(H264Error::Bitstream(..))` unless `condition`.
macro_rules! ensure {
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err($crate::H264Error::Bitstream(format!($($message)+)));
        }
    };
}
pub(crate) use ensure;
