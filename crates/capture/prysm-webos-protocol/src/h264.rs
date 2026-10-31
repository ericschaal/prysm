//! Stateful low-latency codec. Wire pixel format describes decoded RGB24 output.
use crate::{Header, PixelFormat};
use anyhow::{Result, ensure};
use h264_reader::nal::{Nal, RefNal, UnitType, sps::SeqParameterSet};
use openh264::{
    OpenH264API,
    encoder::{
        BitRate, Complexity, EncoderConfig, FrameRate, IntraFramePeriod, Profile, RateControlMode,
        VuiConfig,
    },
    formats::{YUVSlices, YUVSource},
};
use yuv::{BufferStoreMut, YuvConversionMode, YuvPlanarImageMut, YuvRange, YuvStandardMatrix};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    Idr,
    Intra,
    Predicted,
}

pub struct Encoder {
    inner: openh264::encoder::Encoder,
    planes: Vec<u8>,
}
impl Encoder {
    pub fn new(bitrate: u32, fps: u8) -> Result<Self> {
        ensure!(
            (64_000..=100_000_000).contains(&bitrate) && (1..=60).contains(&fps),
            "invalid H.264 rate configuration"
        );
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate))
            .max_frame_rate(FrameRate::from_hz(f32::from(fps)))
            .intra_frame_period(IntraFramePeriod::from_num_frames(u32::from(fps)))
            .complexity(Complexity::Low)
            .profile(Profile::Baseline)
            .rate_control_mode(RateControlMode::Bitrate)
            .skip_frames(true)
            .num_threads(1)
            .vui(VuiConfig::bt601());
        Ok(Self {
            inner: openh264::encoder::Encoder::with_api_config(OpenH264API::from_source(), config)?,
            planes: Vec::new(),
        })
    }
    pub fn encode(
        &mut self,
        header: Header,
        pixels: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<Option<FrameKind>> {
        ensure!(
            pixels.len() == header.raw_len()?,
            "invalid H.264 input length"
        );
        ensure!(
            header.width.is_multiple_of(2) && header.height.is_multiple_of(2),
            "H.264 needs even dimensions"
        );
        let n = header.width as usize * header.height as usize;
        self.planes.resize(n + n / 2, 0);
        let (y, uv) = self.planes.split_at_mut(n);
        let (u, v) = uv.split_at_mut(n / 4);
        if header.format == PixelFormat::Rgb {
            let mut image = YuvPlanarImageMut {
                y_plane: BufferStoreMut::Borrowed(y),
                y_stride: header.width,
                u_plane: BufferStoreMut::Borrowed(u),
                u_stride: header.width / 2,
                v_plane: BufferStoreMut::Borrowed(v),
                v_stride: header.width / 2,
                width: header.width,
                height: header.height,
            };
            yuv::rgb_to_yuv420(
                &mut image,
                pixels,
                header.width * 3,
                YuvRange::Limited,
                YuvStandardMatrix::Bt601,
                YuvConversionMode::Fast,
            )?;
        } else {
            y.copy_from_slice(&pixels[..n]);
            let stride = header.width as usize;
            for row in 0..header.height as usize / 2 {
                for col in 0..stride / 2 {
                    let index = n
                        + row
                            * stride
                            * if header.format == PixelFormat::Nv16 {
                                2
                            } else {
                                1
                            }
                        + col * 2;
                    let dst = row * stride / 2 + col;
                    if header.format == PixelFormat::Nv16 {
                        u[dst] = u8::try_from(
                            (u16::from(pixels[index]) + u16::from(pixels[index + stride]))
                                .div_ceil(2),
                        )?;
                        v[dst] = u8::try_from(
                            (u16::from(pixels[index + 1]) + u16::from(pixels[index + stride + 1]))
                                .div_ceil(2),
                        )?;
                    } else {
                        u[dst] = pixels[index];
                        v[dst] = pixels[index + 1];
                    }
                }
            }
        }
        let source = YUVSlices::new(
            (y, u, v),
            (header.width as usize, header.height as usize),
            (
                header.width as usize,
                header.width as usize / 2,
                header.width as usize / 2,
            ),
        );
        output.clear();
        let bitstream = self.inner.encode(&source)?;
        let kind = match bitstream.frame_type() {
            openh264::encoder::FrameType::IDR => FrameKind::Idr,
            openh264::encoder::FrameType::I => FrameKind::Intra,
            openh264::encoder::FrameType::P => FrameKind::Predicted,
            openh264::encoder::FrameType::Skip => return Ok(None),
            other => anyhow::bail!("unexpected H.264 frame type: {other:?}"),
        };
        bitstream.write_vec(output);
        ensure!(!output.is_empty(), "H.264 encoder returned no access unit");
        Ok(Some(kind))
    }
}

pub struct Decoder(openh264::decoder::Decoder);
impl Decoder {
    pub fn new() -> Result<Self> {
        Ok(Self(openh264::decoder::Decoder::new()?))
    }
    pub fn decode(&mut self, header: Header, payload: &[u8]) -> Result<Vec<u8>> {
        header.validate()?;
        ensure!(
            payload.len() == header.payload_len && header.codec == crate::Codec::H264,
            "invalid H.264 payload"
        );
        // Inspect SPS before native allocation; packet headers cannot constrain an embedded SPS.
        for unit in openh264::nal_units(payload) {
            let bytes = unit
                .get(3..)
                .filter(|bytes| !bytes.is_empty())
                .ok_or_else(|| anyhow::anyhow!("empty H.264 NAL"))?;
            let nal = RefNal::new(bytes, &[], true);
            let kind = nal
                .header()
                .map_err(|error| anyhow::anyhow!("invalid H.264 NAL: {error:?}"))?
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
                "unsupported H.264 NAL type"
            );
            if kind == UnitType::SeqParameterSet {
                let sps = SeqParameterSet::from_bits(nal.rbsp_bits())
                    .map_err(|error| anyhow::anyhow!("invalid H.264 SPS: {error:?}"))?;
                ensure!(
                    sps.pixel_dimensions()
                        .map_err(|error| anyhow::anyhow!("invalid H.264 dimensions: {error:?}"))?
                        == (header.width, header.height),
                    "H.264 SPS dimensions disagree with header"
                );
                ensure!(
                    sps.pic_width_in_mbs_minus1 < 120
                        && sps.pic_height_in_map_units_minus1 < 68
                        && sps.max_num_ref_frames <= 16
                        && matches!(
                            sps.frame_mbs_flags,
                            h264_reader::nal::sps::FrameMbsFlags::Frames
                        )
                        && u8::from(sps.profile_idc) == 66,
                    "unsupported H.264 sequence"
                );
            }
        }
        let decoded = self
            .0
            .decode(payload)?
            .ok_or_else(|| anyhow::anyhow!("H.264 access unit did not produce a frame"))?;
        ensure!(
            decoded.dimensions() == (header.width as usize, header.height as usize),
            "H.264 dimensions disagree with header"
        );
        let mut rgb = vec![0; header.width as usize * header.height as usize * 3];
        let (ys, us, vs) = decoded.strides();
        let image = yuv::YuvPlanarImage {
            y_plane: decoded.y(),
            y_stride: u32::try_from(ys)?,
            u_plane: decoded.u(),
            u_stride: u32::try_from(us)?,
            v_plane: decoded.v(),
            v_stride: u32::try_from(vs)?,
            width: header.width,
            height: header.height,
        };
        yuv::yuv420_to_rgb(
            &image,
            &mut rgb,
            header.width * 3,
            YuvRange::Limited,
            YuvStandardMatrix::Bt601,
        )?;
        Ok(rgb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Codec;
    #[test]
    fn rate_control_skips_inputs_without_breaking_reference_frames() {
        let mut encoder = Encoder::new(64_000, 60).unwrap();
        let mut decoder = Decoder::new().unwrap();
        let input = Header {
            width: 320,
            height: 180,
            format: PixelFormat::Rgb,
            codec: Codec::Raw,
            payload_len: 320 * 180 * 3,
        };
        let mut pixels = vec![0; input.payload_len];
        let mut payload = Vec::new();
        let mut random = 1_u32;
        let mut skipped = 0;
        let mut decoded_after_skip = false;
        for _ in 0..120 {
            for byte in &mut pixels {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *byte = random.to_le_bytes()[3];
            }
            encoder.encode(input, &pixels, &mut payload).unwrap();
            if payload.is_empty() {
                skipped += 1;
                continue;
            }
            let wire = Header {
                codec: Codec::H264,
                payload_len: payload.len(),
                ..input
            };
            decoder.decode(wire, &payload).unwrap();
            decoded_after_skip |= skipped > 0;
        }
        assert!(
            skipped > 0,
            "rate control must be able to skip at a constrained bitrate"
        );
        assert!(
            decoded_after_skip,
            "frames after a skip must remain decodable"
        );
    }

    #[test]
    fn temporal_frames_colors_and_fresh_connection_decode() {
        for format in [PixelFormat::Rgb, PixelFormat::Nv12, PixelFormat::Nv16] {
            let mut encoder = Encoder::new(2_000_000, 30).unwrap();
            let mut decoder = Decoder::new().unwrap();
            for i in 0..35 {
                let mut input = Header {
                    width: 64,
                    height: 34,
                    format,
                    codec: Codec::Raw,
                    payload_len: 0,
                };
                input.payload_len = input.raw_len().unwrap();
                let pixels = match format {
                    PixelFormat::Rgb => [255, 0, 0].repeat(64 * 34),
                    PixelFormat::Nv12 | PixelFormat::Nv16 => [
                        vec![81; 64 * 34],
                        [90, 240].repeat((input.payload_len - 64 * 34) / 2),
                    ]
                    .concat(),
                };
                let mut payload = Vec::new();
                let kind = encoder
                    .encode(input, &pixels, &mut payload)
                    .unwrap()
                    .expect("unconstrained solid frame");
                if i == 0 || i == 30 {
                    assert_eq!(kind, FrameKind::Idr);
                }
                let wire = Header {
                    format: PixelFormat::Rgb,
                    codec: Codec::H264,
                    payload_len: payload.len(),
                    ..input
                };
                let rgb = decoder.decode(wire, &payload).unwrap();
                assert!(
                    rgb.as_chunks::<3>()
                        .0
                        .iter()
                        .all(|p| p[0] > 240 && p[1] < 15 && p[2] < 15),
                    "wrong colors at {format:?}/{i}"
                );
                if i == 0 {
                    assert!(Decoder::new().unwrap().decode(wire, &payload).is_ok());
                    let wrong = Header { width: 32, ..wire };
                    assert!(Decoder::new().unwrap().decode(wrong, &payload).is_err());
                }
            }
        }
    }
}
