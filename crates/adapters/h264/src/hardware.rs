//! One hardware encoder session: the rvenc pipeline plus SPS repair.
use crate::normalize::normalize_access_unit;
use crate::{BITRATE, Backend, FrameKind, H264Error, MAX_ACCESS_UNIT_BYTES};
use prysm_h264_rvenc_sys::{MAX_HEIGHT, MAX_OUTPUT_BYTES, MAX_WIDTH, Pipeline, pitch};

// The hardware backend's largest picture, bitrates and access units are
// rvenc-sys's.
const _: () = {
    let limits = Backend::Hardware.limits();
    assert!(limits.width().max() == MAX_WIDTH && limits.height().max() == MAX_HEIGHT);
    let rvenc = prysm_h264_rvenc_sys::BITRATE;
    assert!(*BITRATE.start() == *rvenc.start() && *BITRATE.end() == *rvenc.end());
    assert!(MAX_ACCESS_UNIT_BYTES == MAX_OUTPUT_BYTES);
};

pub(crate) struct Hardware {
    pipeline: Pipeline,
    width: u32,
    height: u32,
    encoded: Vec<u8>,
    first: bool,
}

impl Hardware {
    pub(crate) fn new(width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self, H264Error> {
        Ok(Self {
            pipeline: Pipeline::new(width, height, pitch(width), fps, bitrate)
                .map_err(H264Error::Hardware)?,
            width,
            height,
            encoded: Vec::new(),
            first: true,
        })
    }

    /// Encode tightly packed NV12; the access unit's SPS is repaired and the
    /// first one must be SPS/PPS/IDR, so it decodes as this picture.
    pub(crate) fn encode(
        &mut self,
        pixels: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<FrameKind, H264Error> {
        self.pipeline
            .encode(pixels, self.width as usize, &mut self.encoded)
            .map_err(H264Error::Hardware)?;
        let kind =
            normalize_access_unit(&self.encoded, self.width, self.height, self.first, output)?;
        self.first = false;
        Ok(kind)
    }
}
