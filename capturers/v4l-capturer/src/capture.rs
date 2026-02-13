use anyhow::{Context, Result};
use async_stream::stream;
use futures::{Stream};
use prysm_capture::{Frame, PixelFormat, PrysmCapturer};
use v4l::buffer::Type;
use v4l::io::traits::CaptureStream;
use v4l::prelude::MmapStream;
use v4l::video::Capture;
use v4l::{Device, Format, FourCC};

pub struct V4lCapturer {
    device: Device,
}

impl V4lCapturer {
    pub fn new(device_path: &str) -> Result<Self> {
        tracing::info!("Opening video device: {}", device_path);
        let device = Device::with_path(device_path).context("Failed to open video device")?;

        Ok(Self {
            device,
        })
    }

    fn create_stream(&mut self, width: u32, height: u32) -> Result<(MmapStream<'_>, Format)> {
        let mut fmt = self.device.format()?;

        fmt.width = width;
        fmt.height = height;

        // Try formats in order of preference (YUYV is smaller and more efficient)
        let preferred_formats = [
            FourCC::new(b"YUYV"), // YUV 4:2:2 (2 bytes/pixel)
            FourCC::new(b"RGB3"), // RGB24 (3 bytes/pixel)
            FourCC::new(b"BGR3"), // BGR24 (3 bytes/pixel)
        ];

        let mut last_error = None;
        for fourcc in preferred_formats {
            fmt.fourcc = fourcc;
            match self.device.set_format(&fmt) {
                Ok(_) => {
                    let format = self.device.format()?;

                    // Validate that a supported format was set
                    if format.fourcc == FourCC::new(b"YUYV")
                        || format.fourcc == FourCC::new(b"RGB3")
                        || format.fourcc == FourCC::new(b"BGR3") {

                        tracing::info!(
                            "Video format set to: {:?} {}x{} (stride: {})",
                            format.fourcc,
                            format.width,
                            format.height,
                            format.stride
                        );

                        let mmap_stream = MmapStream::with_buffers(&self.device, Type::VideoCapture, 4)
                            .context("Failed to create stream")?;

                        return Ok((mmap_stream, format));
                    }
                }
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            }
        }

        anyhow::bail!(
            "Device does not support any of the required formats (YUYV, RGB3, BGR3). Last error: {:?}",
            last_error
        )
    }
}

impl PrysmCapturer for V4lCapturer {
    fn run(&mut self, width: u32, height: u32) -> impl Stream<Item = Frame> + '_ {
        let (mut input_stream, format) = self.create_stream(width, height).expect("Failed to create stream");

        stream! {
            // Determine format and bytes per pixel
            let (pixel_format, bytes_per_pixel) = match format.fourcc.str() {
                Ok("YUYV") => (PixelFormat::YUYV, 2),
                Ok("RGB3") => (PixelFormat::RGB24, 3),
                Ok("BGR3") => (PixelFormat::BGR24, 3),
                _ => {
                    tracing::error!("Unsupported format: {:?}", format.fourcc);
                    return;
                }
            };

            tracing::info!("stream started with format: {:?}", pixel_format);

            loop {
                match input_stream.next() {
                    Ok((buffer, _metadata)) => {
                        // Extract frame data, removing any stride padding
                        let row_size = format.width as usize * bytes_per_pixel;
                        let stride = format.stride as usize;

                        let mut frame_data = Vec::with_capacity(format.height as usize * row_size);

                        for row in 0..format.height as usize {
                            let row_start = row * stride;
                            let row_end = row_start + row_size;
                            frame_data.extend_from_slice(&buffer[row_start..row_end]);
                        }

                        // Emit frame with appropriate format
                        yield Frame::new(frame_data, format.width, format.height, pixel_format);
                    }
                    Err(e) => {
                        tracing::error!("Error capturing frame: {}", e);
                        break;
                    }
                }
            }
        }
    }
}
