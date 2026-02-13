use crate::frames::ColorFrame;
use crate::pipeline::Node;
use prysm_capture::{Frame, PixelFormat};
use prysm_core::Color;

/// Decodes raw pixel data into Color array
#[derive(Debug)]
pub struct PixelDecoder;

impl PixelDecoder {
    pub fn new() -> Self {
        Self
    }

    fn decode_rgb24(&self, data: &[u8], width: u32, height: u32) -> Vec<Color> {
        let pixel_count = (width * height) as usize;
        let mut colors = Vec::with_capacity(pixel_count);

        for i in 0..pixel_count {
            let offset = i * 3;
            if offset + 2 < data.len() {
                colors.push(Color::new(data[offset], data[offset + 1], data[offset + 2]));
            } else {
                colors.push(Color::black());
            }
        }

        colors
    }

    fn decode_yuyv(&self, data: &[u8], width: u32, height: u32) -> Vec<Color> {
        // Use existing prysm_capture::yuyv conversion
        // Convert entire frame at once (SIMD optimized)
        let rgb_data = prysm_capture::yuyv::yuyv_to_rgb(data, width as usize, height as usize);

        let pixel_count = (width * height) as usize;
        let mut colors = Vec::with_capacity(pixel_count);

        for i in 0..pixel_count {
            let offset = i * 3;
            colors.push(Color::new(
                rgb_data[offset],
                rgb_data[offset + 1],
                rgb_data[offset + 2],
            ));
        }

        colors
    }
}

impl Node<Frame, ColorFrame> for PixelDecoder {
    fn process(&mut self, input: Frame) -> ColorFrame {
        let pixels = match input.format {
            PixelFormat::RGB24 => self.decode_rgb24(&input.data, input.width, input.height),
            PixelFormat::YUYV => self.decode_yuyv(&input.data, input.width, input.height),
            PixelFormat::BGR24 | PixelFormat::MJPEG => {
                tracing::error!("{} format not yet supported", input.format);
                vec![Color::black(); (input.width * input.height) as usize]
            }
        };

        ColorFrame::new(pixels, input.width, input.height)
    }
}
