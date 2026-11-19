//! One hardware encoder session: the rvenc pipeline plus SPS repair.
use crate::normalize::normalize_access_unit;
use crate::{FrameKind, H264Error};
use prysm_h264_rvenc_sys::{Pipeline, pitch};

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
