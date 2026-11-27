//! OpenH264: NV12 in, one Baseline access unit out.
use crate::{FrameKind, H264Error};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Complexity, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Profile,
    RateControlMode, VuiConfig,
};
use openh264::formats::YUVSlices;

pub(crate) struct Software {
    inner: openh264::encoder::Encoder,
    chroma: Vec<u8>,
}

impl Software {
    pub(crate) fn new(bitrate: u32, fps: u32) -> Result<Self, H264Error> {
        let rate = fps as f32;
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate))
            .max_frame_rate(FrameRate::from_hz(rate))
            .intra_frame_period(IntraFramePeriod::from_num_frames(0))
            .complexity(Complexity::Low)
            .profile(Profile::Baseline)
            .rate_control_mode(RateControlMode::Bitrate)
            .skip_frames(true)
            .num_threads(1)
            .vui(VuiConfig::bt709());
        Ok(Self {
            inner: openh264::encoder::Encoder::with_api_config(OpenH264API::from_source(), config)?,
            chroma: Vec::new(),
        })
    }

    /// `None` when rate control skipped the picture.
    pub(crate) fn encode(
        &mut self,
        width: usize,
        height: usize,
        y: &[u8],
        uv: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<Option<FrameKind>, H264Error> {
        self.chroma.resize(uv.len(), 0);
        let (u, v) = self.chroma.split_at_mut(uv.len() / 2);
        for (&[cb, cr], (u, v)) in uv
            .as_chunks::<2>()
            .0
            .iter()
            .zip(u.iter_mut().zip(v.iter_mut()))
        {
            *u = cb;
            *v = cr;
        }
        let source = YUVSlices::new((y, u, v), (width, height), (width, width / 2, width / 2));
        output.clear();
        let bitstream = self.inner.encode(&source)?;
        let kind = match bitstream.frame_type() {
            FrameType::IDR => FrameKind::Idr,
            FrameType::I | FrameType::P => FrameKind::Predicted,
            FrameType::Skip => return Ok(None),
            other => return Err(H264Error::FrameType(other)),
        };
        bitstream.write_vec(output);
        if output.is_empty() {
            return Err(H264Error::NoPicture);
        }
        Ok(Some(kind))
    }
}
