//! SPS-only repair for rvenc's progressive Baseline Annex B output.
use super::FrameKind;
use anyhow::{Result, ensure};
use h264_reader::nal::{Nal, RefNal, sps::SeqParameterSet};

// Work in bits only for the tiny parameter set, never for picture data.
struct Bits {
    data: Vec<bool>,
    pos: usize,
}
impl Bits {
    fn read(&mut self, n: usize) -> Result<u32> {
        ensure!(
            n <= 32 && self.pos + n <= self.data.len(),
            "truncated hardware SPS"
        );
        let mut value = 0;
        for bit in &self.data[self.pos..self.pos + n] {
            value = (value << 1) | u32::from(*bit);
        }
        self.pos += n;
        Ok(value)
    }
    fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0;
        while self.read(1)? == 0 {
            zeros += 1;
            ensure!(zeros < 31, "oversized Exp-Golomb value");
        }
        Ok((1 << zeros) - 1 + self.read(zeros)?)
    }
}
fn fixed(out: &mut Vec<bool>, value: u32, n: usize) {
    out.extend((0..n).rev().map(|shift| value & (1 << shift) != 0));
}
fn ue(out: &mut Vec<bool>, value: u32) {
    let value = value + 1;
    let n = (32 - value.leading_zeros()) as usize;
    out.extend(std::iter::repeat_n(false, n - 1));
    fixed(out, value, n);
}
#[allow(clippy::too_many_lines)] // SPS field order is clearer in one checked parser.
fn repair_sps(nal: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    ensure!(nal.len() <= 4096, "oversized hardware SPS");
    let mut rbsp = Vec::new();
    let mut zeros = 0;
    for &byte in &nal[1..] {
        if zeros == 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        rbsp.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    let mut bits = Bits {
        data: rbsp
            .iter()
            .flat_map(|b| (0..8).rev().map(move |i| b & (1 << i) != 0))
            .collect(),
        pos: 0,
    };
    let end = bits
        .data
        .iter()
        .rposition(|b| *b)
        .ok_or_else(|| anyhow::anyhow!("missing SPS trailing bits"))?;
    bits.data.truncate(end);
    ensure!(bits.read(8)? == 66, "hardware SPS must be Baseline");
    bits.read(16)?;
    bits.ue()?; // SPS id
    bits.ue()?; // log2_max_frame_num_minus4
    match bits.ue()? {
        0 => {
            bits.ue()?;
        }
        2 => (),
        _ => anyhow::bail!("unsupported hardware picture order"),
    }
    ensure!(bits.ue()? <= 16, "too many hardware reference frames");
    bits.read(1)?;
    let coded_width = bits
        .ue()?
        .checked_add(1)
        .and_then(|v| v.checked_mul(16))
        .ok_or_else(|| anyhow::anyhow!("SPS width overflow"))?;
    let coded_height = bits
        .ue()?
        .checked_add(1)
        .and_then(|v| v.checked_mul(16))
        .ok_or_else(|| anyhow::anyhow!("SPS height overflow"))?;
    ensure!(bits.read(1)? == 1, "interlaced hardware SPS");
    bits.read(1)?;
    ensure!(
        width > 0
            && height > 0
            && width.is_multiple_of(2)
            && height.is_multiple_of(2)
            && coded_width >= width
            && coded_width - width < 16
            && coded_height >= height
            && coded_height - height < 16,
        "hardware SPS coded dimensions disagree with input"
    );
    let crop_start = bits.pos;
    if bits.read(1)? != 0 {
        for _ in 0..4 {
            bits.ue()?;
        }
    }
    let vui = bits.read(1)? != 0;
    let mut result = bits.data[..crop_start].to_vec();
    result.push(true);
    for v in [0, (coded_width - width) / 2, 0, (coded_height - height) / 2] {
        ue(&mut result, v);
    }
    result.push(true);
    if vui {
        let vui_start = bits.pos;
        if bits.read(1)? != 0 && bits.read(8)? == 255 {
            bits.read(32)?;
        }
        if bits.read(1)? != 0 {
            bits.read(1)?;
        }
        result.extend_from_slice(&bits.data[vui_start..bits.pos]);
        if bits.read(1)? != 0 {
            bits.read(4)?;
            if bits.read(1)? != 0 {
                bits.read(24)?;
            }
        }
    } else {
        result.extend([false, false]);
    }
    result.push(true); // video_signal_type_present_flag
    fixed(&mut result, 5, 3);
    result.extend([false, true]); // limited range, colour_description_present
    for _ in 0..3 {
        fixed(&mut result, 1, 8);
    }
    if vui {
        result.extend_from_slice(&bits.data[bits.pos..]);
    } else {
        result.extend([false; 6]);
    }
    result.push(true);
    while !result.len().is_multiple_of(8) {
        result.push(false);
    }
    let mut output = vec![nal[0]];
    zeros = 0;
    for chunk in result.as_chunks::<8>().0 {
        let byte = chunk.iter().fold(0u8, |v, b| (v << 1) | u8::from(*b));
        if zeros == 2 && byte <= 3 {
            output.push(3);
            zeros = 0;
        }
        output.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    let nal = RefNal::new(&output, &[], true);
    let sps = SeqParameterSet::from_bits(nal.rbsp_bits())
        .map_err(|e| anyhow::anyhow!("invalid repaired SPS: {e:?}"))?;
    ensure!(
        sps.pixel_dimensions()
            .map_err(|e| anyhow::anyhow!("invalid repaired dimensions: {e:?}"))?
            == (width, height),
        "incorrect hardware SPS crop"
    );
    Ok(output)
}

/// Repair metadata only for input already converted to limited-range BT.709.
/// Reject unexpected profiles, dimensions, and access units before transmission.
pub fn normalize_access_unit(
    input: &[u8],
    width: u32,
    height: u32,
    first: bool,
    output: &mut Vec<u8>,
) -> Result<FrameKind> {
    ensure!(
        !input.is_empty() && input.len() <= 8 * 1024 * 1024,
        "invalid hardware access unit length"
    );
    ensure!(
        input.starts_with(&[0, 0, 1]) || input.starts_with(&[0, 0, 0, 1]),
        "hardware output is not Annex B"
    );
    output.clear();
    let mut sps = false;
    let mut pps = false;
    let mut picture = None;
    let mut first_slices = 0;
    for unit in openh264::nal_units(input) {
        let nal = unit
            .get(3..)
            .filter(|n| !n.is_empty())
            .ok_or_else(|| anyhow::anyhow!("empty hardware NAL"))?;
        ensure!(nal[0] & 0x80 == 0, "invalid hardware NAL header");
        let kind = nal[0] & 31;
        output.extend_from_slice(&[0, 0, 1]);
        match kind {
            7 => {
                ensure!(picture.is_none(), "SPS after picture");
                output.extend(repair_sps(nal, width, height)?);
                sps = true;
            }
            8 => {
                ensure!(picture.is_none(), "PPS after picture");
                pps = true;
                output.extend_from_slice(nal);
            }
            1 | 5 => {
                ensure!(
                    picture.is_none_or(|p| p == kind),
                    "mixed hardware picture types"
                );
                picture = Some(kind);
                let mut bits = Bits {
                    data: nal[1..]
                        .iter()
                        .take(16)
                        .flat_map(|b| (0..8).rev().map(move |i| b & (1 << i) != 0))
                        .collect(),
                    pos: 0,
                };
                first_slices += usize::from(bits.ue()? == 0);
                ensure!(
                    matches!(bits.ue()?, 0 | 2 | 5 | 7),
                    "hardware B/SP/SI slice"
                );
                output.extend_from_slice(nal);
            }
            6 | 9 | 12 => output.extend_from_slice(nal),
            _ => anyhow::bail!("unsupported hardware NAL type {kind}"),
        }
    }
    ensure!(
        first_slices == 1,
        "hardware output must contain one complete picture"
    );
    ensure!(
        !first || (sps && pps && picture == Some(5)),
        "hardware stream must start with SPS/PPS/IDR"
    );
    ensure!(
        output.len() <= 8 * 1024 * 1024,
        "repaired hardware access unit too large"
    );
    Ok(if picture == Some(5) {
        FrameKind::Idr
    } else {
        FrameKind::Predicted
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Codec, Header, PixelFormat, h264::Decoder};
    const ORIGINAL: &[u8] = include_bytes!("../../tests/fixtures/rvenc/original-00.au");
    #[test]
    fn original_hardware_sequence_is_repaired_without_reencoding() {
        let mut decoder = Decoder::new().unwrap();
        let header = Header {
            width: 640,
            height: 360,
            format: PixelFormat::Rgb,
            codec: Codec::H264,
            payload_len: ORIGINAL.len(),
        };
        assert!(Decoder::new().unwrap().decode(header, ORIGINAL).is_err());
        let mut output = Vec::new();
        assert_eq!(
            normalize_access_unit(ORIGINAL, 640, 360, true, &mut output).unwrap(),
            FrameKind::Idr
        );
        for (a, b) in openh264::nal_units(ORIGINAL).zip(openh264::nal_units(&output)) {
            if a[3] & 31 != 7 {
                assert_eq!(a, b);
            }
        }
        for i in 0..15 {
            let input = if i == 0 {
                ORIGINAL.to_vec()
            } else {
                std::fs::read(format!(
                    "{}/tests/fixtures/rvenc/adjusted-{i:02}.au",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .unwrap()
            };
            normalize_access_unit(&input, 640, 360, i == 0, &mut output).unwrap();
            let rgb = decoder
                .decode(
                    Header {
                        payload_len: output.len(),
                        ..header
                    },
                    &output,
                )
                .unwrap();
            let expected = u8::try_from(((32 + i * 8 - 16) * 255 + 109) / 219).unwrap();
            assert!(rgb.iter().all(|v| v.abs_diff(expected) <= 3), "frame {i}");
            if i == 0 {
                assert!(
                    Decoder::new()
                        .unwrap()
                        .decode(
                            Header {
                                payload_len: output.len(),
                                ..header
                            },
                            &output
                        )
                        .is_ok()
                );
                let repeated = output.clone();
                normalize_access_unit(&repeated, 640, 360, true, &mut output).unwrap();
                assert_eq!(repeated, output);
            }
        }
    }
    #[test]
    fn rejects_bad_dimensions_truncated_sps_and_multiple_pictures() {
        let mut output = Vec::new();
        for (width, height) in [(638, 360), (640, 359), (640, 370), (1920, 1080)] {
            // A smaller even crop within the last macroblock is representable.
            if width == 638 {
                continue;
            }
            assert!(normalize_access_unit(ORIGINAL, width, height, true, &mut output).is_err());
        }
        for length in 0..ORIGINAL.len().min(20) {
            assert!(
                normalize_access_unit(&ORIGINAL[..length], 640, 360, true, &mut output).is_err()
            );
        }
        let twice = [ORIGINAL, ORIGINAL].concat();
        assert!(normalize_access_unit(&twice, 640, 360, true, &mut output).is_err());
        for height in [354, 358, 360, 368] {
            normalize_access_unit(ORIGINAL, 640, height, true, &mut output).unwrap();
            Decoder::new()
                .unwrap()
                .decode(
                    Header {
                        width: 640,
                        height,
                        format: PixelFormat::Rgb,
                        codec: Codec::H264,
                        payload_len: output.len(),
                    },
                    &output,
                )
                .unwrap();
        }
    }
    #[test]
    #[ignore = "requires PRYSM_HARDWARE_WIRE_DIR from the bounded TV worker probe"]
    fn real_worker_wire_sequences_decode_with_unchanged_receiver() {
        use std::io::Read;
        let root = std::path::PathBuf::from(
            std::env::var_os("PRYSM_HARDWARE_WIRE_DIR").expect("wire fixture directory"),
        );
        for backend in ["hardware", "software"] {
            for size in ["640x360", "320x180"] {
                let mut file =
                    std::fs::File::open(root.join(format!("{backend}-{size}.wire"))).unwrap();
                let mut decoder = Decoder::new().unwrap();
                for index in 0..35 {
                    let mut bytes = [0; crate::HEADER_LEN];
                    file.read_exact(&mut bytes).unwrap();
                    let header = Header::decode(&bytes).unwrap();
                    let mut payload = vec![0; header.payload_len];
                    file.read_exact(&mut payload).unwrap();
                    let rgb = decoder.decode(header, &payload).unwrap();
                    assert_eq!(rgb.len(), (header.width * header.height * 3) as usize);
                    if index == 0 {
                        Decoder::new().unwrap().decode(header, &payload).unwrap();
                    }
                }
                assert_eq!(file.read(&mut [0]).unwrap(), 0);
            }
        }
    }
}
