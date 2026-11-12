//! OpenH264 decoding to NV12, after checking what OpenH264 will allocate.
use crate::{H264Error, MAX_ACCESS_UNIT_BYTES, ensure};
use h264_reader::nal::sps::{FrameMbsFlags, SeqParameterSet};
use h264_reader::nal::{Nal, RefNal, UnitType};
use openh264::formats::YUVSource;

/// One stream's decoder; reference pictures carry over between calls. Create
/// a new one for each encoder session (a [`Picture::keyframe`](crate::Picture)).
pub struct Decoder(openh264::decoder::Decoder);

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Decoder")
    }
}

impl Decoder {
    /// # Errors
    /// OpenH264 refused to initialise.
    pub fn new() -> Result<Self, H264Error> {
        Ok(Self(openh264::decoder::Decoder::new()?))
    }

    /// Decode one access unit of a `width`x`height` stream into `nv12`
    /// (exactly `width * height * 3 / 2` bytes: Y, then interleaved UV).
    ///
    /// The NAL types, and every SPS's size (it must match the header),
    /// profile (Baseline), frame coding and colour signal (limited-range
    /// BT.709) are checked before OpenH264 allocates anything.
    ///
    /// # Errors
    /// A malformed or unsupported access unit, a corrupt bitstream (including
    /// a missing reference picture), or no picture.
    pub fn decode(
        &mut self,
        payload: &[u8],
        width: u32,
        height: u32,
        nv12: &mut [u8],
    ) -> Result<(), H264Error> {
        let (w, h) = (width as usize, height as usize);
        ensure!(
            width.is_multiple_of(2) && height.is_multiple_of(2) && nv12.len() == w * h * 3 / 2,
            "the destination is not {width}x{height} NV12"
        );
        ensure!(
            !payload.is_empty() && payload.len() <= MAX_ACCESS_UNIT_BYTES,
            "access unit of {} bytes",
            payload.len()
        );
        check(payload, width, height)?;
        let decoded = self.0.decode(payload)?.ok_or(H264Error::NoPicture)?;
        ensure!(
            decoded.dimensions() == (w, h),
            "picture is {:?}, the header says {width}x{height}",
            decoded.dimensions()
        );
        let (y_stride, u_stride, v_stride) = decoded.strides();
        let (y, uv) = nv12.split_at_mut(w * h);
        for (row, out) in y.chunks_exact_mut(w).enumerate() {
            out.copy_from_slice(&decoded.y()[row * y_stride..][..w]);
        }
        let (u, v) = (decoded.u(), decoded.v());
        for (row, out) in uv.chunks_exact_mut(w).enumerate() {
            let (u, v) = (&u[row * u_stride..][..w / 2], &v[row * v_stride..][..w / 2]);
            for ((pair, u), v) in out.as_chunks_mut::<2>().0.iter_mut().zip(u).zip(v) {
                pair[0] = *u;
                pair[1] = *v;
            }
        }
        Ok(())
    }
}

/// Inspect every NAL; an SPS must describe exactly this stream.
fn check(payload: &[u8], width: u32, height: u32) -> Result<(), H264Error> {
    ensure!(
        payload.starts_with(&[0, 0, 1]) || payload.starts_with(&[0, 0, 0, 1]),
        "not an Annex B access unit"
    );
    let mut units = 0;
    for unit in openh264::nal_units(payload) {
        units += 1;
        let bytes = unit
            .get(3..)
            .filter(|bytes| !bytes.is_empty())
            .ok_or_else(|| H264Error::Bitstream("empty NAL".into()))?;
        let nal = RefNal::new(bytes, &[], true);
        let kind = nal
            .header()
            .map_err(|error| H264Error::Bitstream(format!("NAL header: {error:?}")))?
            .nal_unit_type();
        ensure!(
            matches!(
                kind,
                UnitType::SliceLayerWithoutPartitioningNonIdr
                    | UnitType::SliceLayerWithoutPartitioningIdr
                    | UnitType::SEI
                    | UnitType::SeqParameterSet
                    | UnitType::PicParameterSet
                    | UnitType::AccessUnitDelimiter
                    | UnitType::FillerData
            ),
            "unsupported NAL type {kind:?}"
        );
        if kind == UnitType::SeqParameterSet {
            let sps = SeqParameterSet::from_bits(nal.rbsp_bits())
                .map_err(|error| H264Error::Bitstream(format!("SPS: {error:?}")))?;
            let size = sps
                .pixel_dimensions()
                .map_err(|error| H264Error::Bitstream(format!("SPS dimensions: {error:?}")))?;
            ensure!(
                size == (width, height),
                "SPS says {}x{}, the header {width}x{height}",
                size.0,
                size.1
            );
            ensure!(
                sps.pic_width_in_mbs_minus1 < 120
                    && sps.pic_height_in_map_units_minus1 < 68
                    && sps.max_num_ref_frames <= 16
                    && matches!(sps.frame_mbs_flags, FrameMbsFlags::Frames)
                    && u8::from(sps.profile_idc) == 66,
                "unsupported sequence (Baseline progressive up to 1920x1088 only)"
            );
            ensure!(
                sps.vui_parameters
                    .as_ref()
                    .and_then(|vui| vui.video_signal_type.as_ref())
                    .is_some_and(|signal| {
                        !signal.video_full_range_flag
                            && signal
                                .colour_description
                                .as_ref()
                                .is_some_and(|color| color.matrix_coefficients == 1)
                    }),
                "the SPS must signal limited-range BT.709"
            );
        }
    }
    ensure!(units > 0, "no NAL units");
    Ok(())
}
