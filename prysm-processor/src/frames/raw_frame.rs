use prysm_capture::PixelFormat;
use std::sync::Arc;

/// Raw frame from capturer (bytes + format)
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub data: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

impl From<prysm_capture::Frame> for RawFrame {
    fn from(frame: prysm_capture::Frame) -> Self {
        Self {
            data: frame.data,
            width: frame.width,
            height: frame.height,
            format: frame.format,
        }
    }
}
