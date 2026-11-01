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
            // TCP preserves references; reconnects already start with a fresh IDR.
            .intra_frame_period(IntraFramePeriod::from_num_frames(0))
            .complexity(Complexity::Low)
            .profile(Profile::Baseline)
            .rate_control_mode(RateControlMode::Bitrate)
            .skip_frames(true)
            .num_threads(1)
            .vui(VuiConfig::bt709());
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
                YuvStandardMatrix::Bt709,
                YuvConversionMode::Balanced,
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
                    "H.264 must signal limited-range BT.709"
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
            YuvStandardMatrix::Bt709,
        )?;
        Ok(rgb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Codec;

    #[test]
    fn rejects_sps_with_wrong_matrix_or_range() {
        for vui in [VuiConfig::bt601(), VuiConfig::bt709_full()] {
            let mut encoder = Encoder::new(2_000_000, 30).unwrap();
            encoder.inner = openh264::encoder::Encoder::with_api_config(
                OpenH264API::from_source(),
                EncoderConfig::new()
                    .profile(Profile::Baseline)
                    .max_frame_rate(FrameRate::from_hz(30.0))
                    .vui(vui),
            )
            .unwrap();
            let input = Header {
                width: 32,
                height: 32,
                format: PixelFormat::Rgb,
                codec: Codec::Raw,
                payload_len: 32 * 32 * 3,
            };
            let mut payload = Vec::new();
            encoder
                .encode(input, &vec![0; input.payload_len], &mut payload)
                .unwrap();
            let wire = Header {
                codec: Codec::H264,
                payload_len: payload.len(),
                ..input
            };
            let error = Decoder::new().unwrap().decode(wire, &payload).unwrap_err();
            assert!(
                error.to_string().contains("limited-range BT.709"),
                "{error}"
            );
        }
    }

    #[test]
    fn constrained_bitrate_preserves_color_and_shadow_patch_averages() {
        let colors = [
            [0, 0, 0],
            [4, 4, 4],
            [8, 8, 8],
            [16, 16, 16],
            [32, 32, 32],
            [128, 128, 128],
            [235, 235, 235],
            [255, 255, 255],
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 0],
            [0, 255, 255],
            [255, 0, 255],
            [180, 95, 55],
            [24, 40, 72],
        ];
        let input = Header {
            width: 256,
            height: 128,
            format: PixelFormat::Rgb,
            codec: Codec::Raw,
            payload_len: 256 * 128 * 3,
        };
        let pixels: Vec<u8> = (0..128)
            .flat_map(|y| (0..256).flat_map(move |x| colors[y / 32 * 4 + x / 64]))
            .collect();
        let mut encoder = Encoder::new(64_000, 30).unwrap();
        let mut decoder = Decoder::new().unwrap();
        let mut payload = Vec::new();
        let mut total_bytes = 0;
        let mut max_error = 0.0_f64;
        for _ in 0..90 {
            if encoder
                .encode(input, &pixels, &mut payload)
                .unwrap()
                .is_none()
            {
                continue;
            }
            total_bytes += payload.len();
            let rgb = decoder
                .decode(
                    Header {
                        codec: Codec::H264,
                        payload_len: payload.len(),
                        ..input
                    },
                    &payload,
                )
                .unwrap();
            for (patch, color) in colors.iter().enumerate() {
                // Average patch interiors as LED sampling does, excluding subsampled boundaries.
                let mut sums = [0_u32; 3];
                for y in patch / 4 * 32 + 4..patch / 4 * 32 + 28 {
                    for x in patch % 4 * 64 + 4..patch % 4 * 64 + 60 {
                        for channel in 0..3 {
                            sums[channel] += u32::from(rgb[(y * 256 + x) * 3 + channel]);
                        }
                    }
                }
                for channel in 0..3 {
                    let error = (f64::from(sums[channel]) / (24.0 * 56.0)
                        - f64::from(color[channel]))
                    .abs();
                    max_error = max_error.max(error);
                    assert!(error <= 6.0, "patch {color:?}, channel {channel}: {error}");
                }
            }
        }
        assert!(total_bytes > 0);
        eprintln!(
            "color patches: {total_bytes} bytes / 3 s, max mean RGB error {max_error:.2}/255"
        );
    }

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
                        vec![63; 64 * 34],
                        [102, 240].repeat((input.payload_len - 64 * 34) / 2),
                    ]
                    .concat(),
                };
                let mut payload = Vec::new();
                let kind = encoder
                    .encode(input, &pixels, &mut payload)
                    .unwrap()
                    .expect("unconstrained solid frame");
                assert_eq!(
                    kind,
                    if i == 0 {
                        FrameKind::Idr
                    } else {
                        FrameKind::Predicted
                    }
                );
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
