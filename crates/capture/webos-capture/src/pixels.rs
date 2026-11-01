use anyhow::{Result, ensure};
use prysm_metrics::{Content, Stage};
use prysm_webos_protocol::{Header, PixelFormat};
use yuv::{YuvBiPlanarImage, YuvConversionMode, YuvRange, YuvStandardMatrix};

/// Owned reusable composition storage; returned bytes retain their wire metadata.
#[derive(Default)]
pub struct CompositionBuffer {
    rgb: Vec<u8>,
}

pub struct ComposedFrame<'a> {
    pub header: Header,
    pub pixels: &'a [u8],
    pub content: Content,
}

impl CompositionBuffer {
    pub fn compose<'a>(
        &'a mut self,
        width: u32,
        height: u32,
        video: Option<(Header, &'a [u8])>,
        ui: &Bgra8Pixels,
    ) -> Result<ComposedFrame<'a>> {
        let output_header = header(width, height, PixelFormat::Rgb)?;
        ensure!(
            ui.bytes.len() == output_header.raw_len()? / 3 * 4,
            "UI and video dimensions differ"
        );
        if let Some((video_header, pixels)) = video {
            ensure!(
                video_header.width == width
                    && video_header.height == height
                    && pixels.len() == video_header.raw_len()?,
                "invalid video plane"
            );
            if ui
                .bytes
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 0)
            {
                return Ok(ComposedFrame {
                    header: video_header,
                    pixels,
                    content: Content::Video,
                });
            }
        }
        {
            let _time = prysm_metrics::timer(Stage::CompositionConvert);
            if let Some((video_header, pixels)) = video {
                to_rgb(video_header, pixels, &mut self.rgb)?;
            } else {
                ui.to_rgb(width, height, &mut self.rgb)?;
            }
        }
        if video.is_some() {
            let _time = prysm_metrics::timer(Stage::CompositionBlend);
            ui.blend_into(&mut self.rgb)?;
        }
        Ok(ComposedFrame {
            header: output_header,
            pixels: &self.rgb,
            content: if video.is_some() {
                Content::Blended
            } else {
                Content::Ui
            },
        })
    }
}

/// LG's packed, premultiplied BGRA8 UI surface. The type retains the
/// capture/conversion contract even when its byte allocation is reused.
#[derive(Default)]
pub struct Bgra8Pixels {
    bytes: Vec<u8>,
}
impl Bgra8Pixels {
    pub fn bytes_mut(&mut self) -> &mut Vec<u8> {
        &mut self.bytes
    }
    pub fn blend_into(&self, rgb: &mut [u8]) -> Result<()> {
        blend(rgb, &self.bytes)
    }
    pub fn to_rgb(&self, width: u32, height: u32, rgb: &mut Vec<u8>) -> Result<()> {
        rgb.resize(width as usize * height as usize * 3, 0);
        yuv::bgra_to_rgb(&self.bytes, width * 4, rgb, width * 3, width, height)?;
        Ok(())
    }
}
impl From<&Bgra8Pixels> for prysm_metrics::Format {
    fn from(_: &Bgra8Pixels) -> Self {
        Self::Bgra
    }
}

pub fn header(width: u32, height: u32, format: PixelFormat) -> Result<Header> {
    let mut header = Header {
        width,
        height,
        format,
        codec: prysm_webos_protocol::Codec::Raw,
        payload_len: 0,
    };
    header.payload_len = header.raw_len()?;
    Ok(header)
}

/// Copy active rows without padding, zero-filling, or per-frame allocation.
pub fn copy_plane(
    source: &[u8],
    stride: usize,
    row_bytes: usize,
    rows: usize,
    output: &mut Vec<u8>,
) -> Result<()> {
    ensure!(
        rows > 0 && rows <= 1080 && row_bytes > 0 && stride >= row_bytes && stride <= 32768,
        "invalid native plane geometry"
    );
    ensure!(
        source.len() >= stride * (rows - 1) + row_bytes,
        "short native plane"
    );
    output.reserve(row_bytes * rows);
    for row in source.chunks(stride).take(rows) {
        output.extend_from_slice(&row[..row_bytes]);
    }
    Ok(())
}

pub fn to_rgb(header: Header, input: &[u8], rgb: &mut Vec<u8>) -> Result<()> {
    ensure!(
        input.len() == header.raw_len()?,
        "incorrect native payload length"
    );
    rgb.resize(header.width as usize * header.height as usize * 3, 0);
    if header.format == PixelFormat::Rgb {
        rgb.copy_from_slice(input);
        return Ok(());
    }
    let (y, uv) = input.split_at(header.width as usize * header.height as usize);
    let image = YuvBiPlanarImage {
        y_plane: y,
        y_stride: header.width,
        uv_plane: uv,
        uv_stride: header.width,
        width: header.width,
        height: header.height,
    };
    let convert = match header.format {
        PixelFormat::Nv12 => yuv::yuv_nv12_to_rgb,
        PixelFormat::Nv16 => yuv::yuv_nv16_to_rgb,
        PixelFormat::Rgb => unreachable!(),
    };
    // The default display tap on the tested TV captures limited-range BT.709.
    convert(
        &image,
        rgb,
        header.width * 3,
        YuvRange::Limited,
        YuvStandardMatrix::Bt709,
        YuvConversionMode::Balanced,
    )?;
    Ok(())
}

/// LG UI surfaces are little-endian ARGB (BGRA bytes), with premultiplied alpha.
pub fn blend(rgb: &mut [u8], bgra: &[u8]) -> Result<()> {
    ensure!(
        rgb.len().is_multiple_of(3)
            && bgra.len() / 4 == rgb.len() / 3
            && bgra.len().is_multiple_of(4),
        "UI and video dimensions differ"
    );
    for (video, ui) in rgb
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(bgra.as_chunks::<4>().0)
    {
        let inverse = 256 - u16::from(ui[3]);
        for (dst, src) in video.iter_mut().zip([ui[2], ui[1], ui[0]]) {
            *dst = (u16::from(src) + ((u16::from(*dst) * inverse) >> 8)).min(255) as u8;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_blends_rgb_and_reuses_storage() {
        let mut ui = Bgra8Pixels::default();
        *ui.bytes_mut() = [0, 0, 128, 128].repeat(8);
        let mut buffer = CompositionBuffer::default();
        let video = [0, 0, 255].repeat(8);
        let video_header = header(4, 2, PixelFormat::Rgb).unwrap();
        let frame = buffer
            .compose(4, 2, Some((video_header, &video)), &ui)
            .unwrap();
        assert_eq!(frame.header.format, PixelFormat::Rgb);
        assert_eq!(frame.pixels, [128, 0, 127].repeat(8));
        let allocation = frame.pixels.as_ptr();
        let frame = buffer
            .compose(4, 2, Some((video_header, &video)), &ui)
            .unwrap();
        assert_eq!(frame.pixels.as_ptr(), allocation);
    }

    #[test]
    fn transparent_ui_preserves_native_video_and_visible_pixels_restore_composition() {
        for format in [PixelFormat::Nv12, PixelFormat::Nv16, PixelFormat::Rgb] {
            let video_header = header(4, 2, format).unwrap();
            let mut video = vec![96; video_header.raw_len().unwrap()];
            if format != PixelFormat::Rgb {
                video[8..].fill(128);
            }
            let mut ui = Bgra8Pixels::default();
            *ui.bytes_mut() = vec![0; 4 * 2 * 4];
            let mut buffer = CompositionBuffer::default();
            let frame = buffer
                .compose(4, 2, Some((video_header, &video)), &ui)
                .unwrap();
            assert_eq!(frame.header.format, format);
            assert_eq!(frame.content, Content::Video);
            assert_eq!(frame.pixels, video);
            assert_eq!(frame.pixels.as_ptr(), video.as_ptr());

            // Even the last pixel with minimal alpha must take the composition path.
            ui.bytes_mut()[28..].copy_from_slice(&[0, 0, 1, 1]);
            let frame = buffer
                .compose(4, 2, Some((video_header, &video)), &ui)
                .unwrap();
            assert_eq!(frame.header.format, PixelFormat::Rgb);
            assert_ne!(frame.pixels.as_ptr(), video.as_ptr());
            assert_eq!(frame.content, Content::Blended);
            ui.bytes_mut()[28..].copy_from_slice(&[0, 0, 255, 255]);
            let frame = buffer
                .compose(4, 2, Some((video_header, &video)), &ui)
                .unwrap();
            assert_eq!(&frame.pixels[21..], &[255, 0, 0]);

            ui.bytes_mut().fill(0);
            let frame = buffer
                .compose(4, 2, Some((video_header, &video)), &ui)
                .unwrap();
            assert_eq!(frame.header.format, format);
            assert_eq!(frame.pixels.as_ptr(), video.as_ptr());
            let frame = buffer.compose(4, 2, None, &ui).unwrap();
            assert_eq!(frame.header.format, PixelFormat::Rgb);
            assert_eq!(frame.pixels, [0; 24]);
            assert_eq!(frame.content, Content::Ui);
        }
    }

    #[test]
    fn ui_transitions_keep_software_h264_references_and_colors() {
        use crate::encoder::{Encoder, H264_BITRATE};
        use prysm_webos_protocol::{Compression, h264};
        for format in [PixelFormat::Nv12, PixelFormat::Nv16] {
            let video_header = header(64, 32, format).unwrap();
            let mut video = vec![96; video_header.raw_len().unwrap()];
            video[64 * 32..].fill(128);
            let mut ui = Bgra8Pixels::default();
            let mut buffer = CompositionBuffer::default();
            let mut encoder = Encoder::new(Compression::H264Software, 30, H264_BITRATE).unwrap();
            let mut decoder = h264::Decoder::new().unwrap();
            for (alpha, expected) in [
                (0, [93, 93, 93]),
                (255, [255, 0, 0]),
                (128, [174, 46, 46]),
                (0, [93, 93, 93]),
            ] {
                *ui.bytes_mut() = [0, 0, alpha, alpha].repeat(64 * 32);
                let frame = buffer
                    .compose(64, 32, Some((video_header, &video)), &ui)
                    .unwrap();
                let packet = encoder
                    .encode(frame.header, frame.pixels, frame.content)
                    .unwrap()
                    .unwrap();
                let rgb = decoder.decode(packet.header, packet.pixels).unwrap();
                assert!(rgb.as_chunks::<3>().0.iter().all(|pixel| {
                    pixel
                        .iter()
                        .zip(expected)
                        .all(|(value, expected)| value.abs_diff(expected) <= 8)
                }));
            }
        }
    }

    #[test]
    fn composition_rejects_invalid_geometry_and_short_planes() {
        let mut buffer = CompositionBuffer::default();
        let mut ui = Bgra8Pixels::default();
        assert!(buffer.compose(0, 2, None, &ui).is_err());
        assert!(buffer.compose(2, 0, None, &ui).is_err());
        assert!(buffer.compose(2, 2, None, &ui).is_err());
        *ui.bytes_mut() = [0; 4].repeat(4);
        assert!(
            buffer
                .compose(
                    2,
                    2,
                    Some((header(4, 2, PixelFormat::Rgb).unwrap(), &[0; 24])),
                    &ui
                )
                .is_err()
        );
        assert!(
            buffer
                .compose(
                    2,
                    2,
                    Some((header(2, 2, PixelFormat::Rgb).unwrap(), &[0; 3])),
                    &ui
                )
                .is_err()
        );
        ui.bytes_mut().pop();
        assert!(
            buffer
                .compose(
                    2,
                    2,
                    Some((header(2, 2, PixelFormat::Nv12).unwrap(), &[0; 6])),
                    &ui
                )
                .is_err()
        );
    }

    #[test]
    fn ui_payload_keeps_bgra_contract_through_conversion() {
        let mut ui = Bgra8Pixels::default();
        ui.bytes_mut().extend_from_slice(&[0, 0, 255, 255]);
        assert_eq!(
            prysm_metrics::Format::from(&ui),
            prysm_metrics::Format::Bgra
        );
        let mut rgb = Vec::new();
        ui.to_rgb(1, 1, &mut rgb).unwrap();
        assert_eq!(rgb, [255, 0, 0]);
    }

    #[test]
    fn copies_padded_rows_and_rejects_invalid_planes() {
        let input = [1, 2, 3, 99, 4, 5, 6, 99];
        let mut output = Vec::new();
        copy_plane(&input, 4, 3, 2, &mut output).unwrap();
        assert_eq!(output, [1, 2, 3, 4, 5, 6]);
        assert!(copy_plane(&input[..2], 4, 3, 2, &mut output).is_err());
        assert!(copy_plane(&input, 0, 3, 2, &mut output).is_err());
    }

    #[test]
    fn nv12_black_white_and_premultiplied_ui() {
        let h = header(2, 2, PixelFormat::Nv12).unwrap();
        let mut rgb = Vec::new();
        to_rgb(h, &[16, 235, 16, 235, 128, 128], &mut rgb).unwrap();
        assert!(rgb[..3].iter().all(|v| *v <= 1));
        assert!(rgb[3..6].iter().all(|v| *v >= 253));
        to_rgb(h, &[63, 63, 63, 63, 102, 240], &mut rgb).unwrap();
        assert!(
            rgb.as_chunks::<3>()
                .0
                .iter()
                .all(|pixel| pixel[0] >= 250 && pixel[1] <= 3 && pixel[2] <= 3)
        );
        let mut rgb = [100; 9];
        blend(&mut rgb, &[0, 0, 0, 0, 0, 0, 128, 128, 0, 0, 255, 255]).unwrap();
        assert_eq!(rgb, [100, 100, 100, 178, 50, 50, 255, 0, 0]);
        assert!(blend(&mut rgb, &[0; 4]).is_err());
    }
}
