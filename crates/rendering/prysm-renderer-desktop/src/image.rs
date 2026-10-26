//! Decode capture pixels for the egui texture.
use prysm_capture::{Frame, PixelFormat};

pub(super) fn frame_to_image(frame: &Frame, rgb_scratch: &mut Vec<u8>) -> Option<egui::ColorImage> {
    let rgb_data: &[u8] = match frame.format {
        PixelFormat::RGB24 => &frame.data,
        PixelFormat::BGR24 => {
            rgb_scratch.clone_from(&frame.data);
            for pixel in rgb_scratch.chunks_exact_mut(3) {
                pixel.swap(0, 2);
            }
            rgb_scratch
        }
        PixelFormat::YUYV => {
            prysm_capture::yuyv::yuyv_to_rgb_into(
                &frame.data,
                rgb_scratch,
                frame.width as usize,
                frame.height as usize,
                frame.yuv_range,
                frame.yuv_matrix,
            );
            rgb_scratch
        }
        PixelFormat::MJPEG => return None,
    };
    Some(egui::ColorImage::from_rgb(
        [frame.width as usize, frame.height as usize],
        rgb_data,
    ))
}
