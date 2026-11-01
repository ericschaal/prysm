//! Decode capture pixels for the egui texture.
use prysm_core::{Frame, PixelFormat};

pub(super) fn frame_to_image(frame: &Frame, rgb_scratch: &mut Vec<u8>) -> Option<egui::ColorImage> {
    let rgb_data: &[u8] = match frame.format() {
        PixelFormat::RGB24 => frame.as_slice(),
        PixelFormat::BGR24 => {
            frame.as_slice().clone_into(rgb_scratch);
            for pixel in rgb_scratch.chunks_exact_mut(3) {
                pixel.swap(0, 2);
            }
            rgb_scratch
        }
        PixelFormat::YUYV { .. } => {
            prysm_core::yuyv::yuyv_to_rgb_into(frame, rgb_scratch).ok()?;
            rgb_scratch
        }
        PixelFormat::MJPEG => return None,
    };
    Some(egui::ColorImage::from_rgb(
        [frame.width() as usize, frame.height() as usize],
        rgb_data,
    ))
}
