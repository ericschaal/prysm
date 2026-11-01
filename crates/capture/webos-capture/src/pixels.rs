use crate::cli::CompositionFormat;
use anyhow::{Result, ensure};
use prysm_metrics::Stage;
use prysm_webos_protocol::{Header, PixelFormat};
use yuv::{YuvBiPlanarImage, YuvConversionMode, YuvRange, YuvStandardMatrix};

/// Owned reusable composition storage; returned bytes retain their wire metadata.
#[derive(Default)]
pub struct CompositionBuffer {
    rgb: Vec<u8>,
    nv12: Vec<u8>,
}

pub struct ComposedFrame<'a> {
    pub header: Header,
    pub pixels: &'a [u8],
}

impl From<CompositionFormat> for PixelFormat {
    fn from(format: CompositionFormat) -> Self {
        match format {
            CompositionFormat::Rgb => Self::Rgb,
            CompositionFormat::Nv12 => Self::Nv12,
        }
    }
}

impl CompositionBuffer {
    pub fn compose(
        &mut self,
        width: u32,
        height: u32,
        video: Option<(Header, &[u8])>,
        ui: &Bgra8Pixels,
        format: CompositionFormat,
    ) -> Result<ComposedFrame<'_>> {
        // Validate subsampling constraints before modifying the composition.
        let output_header = header(width, height, format.into())?;
        {
            let _time = prysm_metrics::timer(Stage::CompositionConvert);
            if let Some((video_header, pixels)) = video {
                ensure!(
                    video_header.width == width && video_header.height == height,
                    "UI and video dimensions differ"
                );
                to_rgb(video_header, pixels, &mut self.rgb)?;
            } else {
                ui.to_rgb(width, height, &mut self.rgb)?;
            }
        }
        if video.is_some() {
            let _time = prysm_metrics::timer(Stage::CompositionBlend);
            ui.blend_into(&mut self.rgb)?;
        }
        let pixels = match format {
            CompositionFormat::Rgb => self.rgb.as_slice(),
            CompositionFormat::Nv12 => {
                let _time = prysm_metrics::timer(Stage::CompositionToNv12);
                self.nv12.resize(output_header.payload_len, 0);
                let (y, uv) = self.nv12.split_at_mut(width as usize * height as usize);
                let mut image = yuv::YuvBiPlanarImageMut {
                    y_plane: yuv::BufferStoreMut::Borrowed(y),
                    y_stride: width,
                    uv_plane: yuv::BufferStoreMut::Borrowed(uv),
                    uv_stride: width,
                    width,
                    height,
                };
                // Match the receiver's limited-range BT.601 interpretation.
                yuv::rgb_to_yuv_nv12(
                    &mut image,
                    &self.rgb,
                    width * 3,
                    YuvRange::Limited,
                    YuvStandardMatrix::Bt601,
                    YuvConversionMode::Fast,
                )?;
                self.nv12.as_slice()
            }
        };
        Ok(ComposedFrame {
            header: output_header,
            pixels,
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
    // Match upstream libyuv's default NV12/NV16 conversion (limited BT.601).
    convert(
        &image,
        rgb,
        header.width * 3,
        YuvRange::Limited,
        YuvStandardMatrix::Bt601,
        YuvConversionMode::Fast,
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
    fn nv12_blend_matches_rgb_for_uniform_chroma_and_reuses_storage() {
        let width = 4;
        let height = 2;
        let video = [0, 0, 255].repeat(8);
        let mut ui = Bgra8Pixels::default();
        *ui.bytes_mut() = [0, 0, 128, 128].repeat(8);
        let mut buffer = CompositionBuffer::default();
        let video_header = header(width, height, PixelFormat::Rgb).unwrap();
        let expected = buffer
            .compose(
                width,
                height,
                Some((video_header, &video)),
                &ui,
                CompositionFormat::Rgb,
            )
            .unwrap()
            .pixels
            .to_vec();
        assert_eq!(expected, [128, 0, 127].repeat(8));
        let composed = buffer
            .compose(
                width,
                height,
                Some((video_header, &video)),
                &ui,
                CompositionFormat::Nv12,
            )
            .unwrap();
        assert_eq!(composed.header.format, PixelFormat::Nv12);
        assert_eq!(composed.pixels.len() * 2, expected.len());
        let allocation = composed.pixels.as_ptr();
        let mut decoded = Vec::new();
        to_rgb(composed.header, composed.pixels, &mut decoded).unwrap();
        assert!(
            decoded
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.abs_diff(*b) <= 4)
        );
        let composed = buffer
            .compose(
                width,
                height,
                Some((video_header, &video)),
                &ui,
                CompositionFormat::Nv12,
            )
            .unwrap();
        assert_eq!(composed.pixels.as_ptr(), allocation);
    }

    #[test]
    fn nv12_keeps_uv_order_and_black_white_levels() {
        let mut ui = Bgra8Pixels::default();
        *ui.bytes_mut() = [0; 4].repeat(8);
        let mut buffer = CompositionBuffer::default();
        for row in [
            [255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255],
            [0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255],
        ] {
            let video = row.repeat(2);
            let composed = buffer
                .compose(
                    4,
                    2,
                    Some((header(4, 2, PixelFormat::Rgb).unwrap(), &video)),
                    &ui,
                    CompositionFormat::Nv12,
                )
                .unwrap();
            let mut rgb = Vec::new();
            to_rgb(composed.header, composed.pixels, &mut rgb).unwrap();
            assert!(
                rgb.iter().zip(&video).all(|(a, b)| a.abs_diff(*b) <= 4),
                "{rgb:?}"
            );
        }
    }

    #[test]
    fn composition_rejects_invalid_geometry_and_short_planes() {
        let mut buffer = CompositionBuffer::default();
        let mut ui = Bgra8Pixels::default();
        assert!(
            buffer
                .compose(3, 2, None, &ui, CompositionFormat::Nv12)
                .is_err()
        );
        assert!(
            buffer
                .compose(2, 3, None, &ui, CompositionFormat::Nv12)
                .is_err()
        );
        assert!(
            buffer
                .compose(2, 2, None, &ui, CompositionFormat::Nv12)
                .is_err()
        );
        *ui.bytes_mut() = [0; 4].repeat(4);
        assert!(
            buffer
                .compose(
                    2,
                    2,
                    Some((header(4, 2, PixelFormat::Rgb).unwrap(), &[0; 24])),
                    &ui,
                    CompositionFormat::Nv12
                )
                .is_err()
        );
        assert!(
            buffer
                .compose(
                    2,
                    2,
                    Some((header(2, 2, PixelFormat::Rgb).unwrap(), &[0; 3])),
                    &ui,
                    CompositionFormat::Nv12
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
        to_rgb(h, &[81, 81, 81, 81, 90, 240], &mut rgb).unwrap();
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
