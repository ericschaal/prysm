use anyhow::{Result, ensure};
use prysm_metrics::Stage;
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
}

impl CompositionBuffer {
    pub fn compose(
        &mut self,
        width: u32,
        height: u32,
        video: Option<(Header, &[u8])>,
        ui: &Bgra8Pixels,
    ) -> Result<ComposedFrame<'_>> {
        let output_header = header(width, height, PixelFormat::Rgb)?;
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
        Ok(ComposedFrame {
            header: output_header,
            pixels: &self.rgb,
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
