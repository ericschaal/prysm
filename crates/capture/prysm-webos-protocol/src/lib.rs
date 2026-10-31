//! Bounded webOS frames with an explicit wire codec.
#[cfg(feature = "h264")]
pub mod h264;
use anyhow::{Result, bail, ensure};

pub const HEADER_LEN: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    Rgb = 0,
    Nv12 = 1,
    Nv16 = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Codec {
    Raw = 0,
    Lz4 = 1,
    H264 = 2,
}

#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub codec: Codec,
    pub payload_len: usize,
}

impl Header {
    pub fn raw_len(self) -> Result<usize> {
        ensure!(
            (1..=1920).contains(&self.width) && (1..=1080).contains(&self.height),
            "invalid webOS frame dimensions"
        );
        let pixels = self.width as usize * self.height as usize;
        Ok(match self.format {
            PixelFormat::Rgb => pixels * 3,
            PixelFormat::Nv12 | PixelFormat::Nv16 => {
                ensure!(self.width.is_multiple_of(2), "odd webOS chroma width");
                if self.format == PixelFormat::Nv12 {
                    ensure!(self.height.is_multiple_of(2), "odd webOS chroma height");
                    pixels + pixels / 2
                } else {
                    pixels * 2
                }
            }
        })
    }

    pub fn validate(self) -> Result<Self> {
        let raw_len = self.raw_len()?;
        ensure!(
            match self.codec {
                Codec::Raw => self.payload_len == raw_len,
                Codec::Lz4 => self.payload_len > 0 && self.payload_len < raw_len,
                Codec::H264 =>
                    self.format == PixelFormat::Rgb
                        && self.width.is_multiple_of(2)
                        && self.height.is_multiple_of(2)
                        && (1..=8_388_608).contains(&self.payload_len),
            },
            "invalid webOS payload length"
        );
        Ok(self)
    }

    pub fn encode(self) -> Result<[u8; HEADER_LEN]> {
        self.validate()?;
        let mut bytes = [0; HEADER_LEN];
        bytes[..4].copy_from_slice(b"PRY2");
        for (offset, value) in [
            (4, self.width),
            (8, self.height),
            (12, self.format as u32),
            (16, self.codec as u32),
            (20, u32::try_from(self.payload_len)?),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8; HEADER_LEN]) -> Result<Self> {
        ensure!(&bytes[..4] == b"PRY2", "unsupported webOS frame protocol");
        let field = |offset| {
            u32::from_be_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        let format = match field(12) {
            0 => PixelFormat::Rgb,
            1 => PixelFormat::Nv12,
            2 => PixelFormat::Nv16,
            _ => bail!("unsupported webOS pixel format"),
        };
        let codec = match field(16) {
            0 => Codec::Raw,
            1 => Codec::Lz4,
            2 => Codec::H264,
            _ => bail!("unsupported webOS compression"),
        };
        Self {
            width: field(4),
            height: field(8),
            format,
            codec,
            payload_len: field(20) as usize,
        }
        .validate()
    }
}

/// Reuse the output allocation; send raw when LZ4 would expand the frame.
pub fn compress(input: &[u8], scratch: &mut Vec<u8>) -> Result<Option<usize>> {
    scratch.resize(lz4_flex::block::get_maximum_output_size(input.len()), 0);
    let length = lz4_flex::block::compress_into(input, scratch)?;
    Ok((length < input.len()).then_some(length))
}

/// The caller sizes output from validated dimensions, never from the LZ4 block.
pub fn decompress(input: &[u8], output: &mut [u8]) -> Result<()> {
    let length = lz4_flex::block::decompress_into(input, output)?;
    ensure!(
        length == output.len(),
        "incorrect webOS decompressed length"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_formats_and_reject_unbounded_headers() {
        for format in [PixelFormat::Rgb, PixelFormat::Nv12, PixelFormat::Nv16] {
            let mut header = Header {
                width: 4,
                height: 2,
                format,
                codec: Codec::Raw,
                payload_len: 0,
            };
            header.payload_len = header.raw_len().unwrap();
            assert_eq!(
                Header::decode(&header.encode().unwrap()).unwrap().format,
                format
            );
            header.payload_len += 1;
            assert!(header.encode().is_err());
            header.width = u32::MAX;
            assert!(header.raw_len().is_err());
        }
        assert!(Header::decode(&[255; HEADER_LEN]).is_err());
    }

    #[test]
    fn h264_headers_bound_access_units_and_allow_keyframe_overhead() {
        let header = Header {
            width: 16,
            height: 16,
            format: PixelFormat::Rgb,
            codec: Codec::H264,
            payload_len: 2048,
        };
        assert_eq!(
            Header::decode(&header.encode().unwrap()).unwrap().codec,
            Codec::H264
        );
        for invalid in [
            Header {
                payload_len: 0,
                ..header
            },
            Header {
                payload_len: 8_388_609,
                ..header
            },
            Header {
                width: 15,
                ..header
            },
            Header {
                format: PixelFormat::Nv12,
                ..header
            },
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn lz4_is_lossless_bounded_and_falls_back_for_noise() {
        let input = vec![42; 320 * 180 * 3];
        let mut scratch = Vec::new();
        let length = compress(&input, &mut scratch).unwrap().unwrap();
        let mut output = vec![0; input.len()];
        decompress(&scratch[..length], &mut output).unwrap();
        assert_eq!(input, output);
        assert!(decompress(&scratch[..length], &mut [0; 8]).is_err());
        assert!(decompress(&[0xff], &mut output).is_err());
        assert!(decompress(&scratch[..length], &mut vec![0; input.len() + 1]).is_err());
        assert!(compress(&[0, 1, 2], &mut scratch).unwrap().is_none());
    }
}
