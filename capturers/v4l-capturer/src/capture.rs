use anyhow::{Context, Result};
use async_stream::stream;
use futures::{Stream};
use prysm_capture::{Frame,PrysmCapturer};
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
        fmt.fourcc = FourCC::new(b"RGB3"); // Try RGB24 first

        self.device
            .set_format(&fmt)
            .context("Format not supported")?;

        let format = self.device.format()?;
        tracing::info!(
            "Video format set to: {:?} {}x{}",
            fmt.fourcc,
            fmt.width,
            fmt.height
        );

        let mmap_stream = MmapStream::with_buffers(&self.device, Type::VideoCapture, 4)
            .context("Failed to create stream")?;

        Ok((mmap_stream, format))
    }
}

impl PrysmCapturer for V4lCapturer {
    fn run(&mut self, width: u32, height: u32) -> impl Stream<Item = Frame> + '_ {
        let (mut input_stream, format) = self.create_stream(width, height).expect("Failed to create stream");
        tracing::info!("stream started with format: {:?}", format);

        stream! {

            loop {
                match input_stream.next() {
                    Ok((buffer, _metadata)) => {
                        // Extract actual RGB data, removing any padding
                        let bytes_per_pixel = 3; // RGB3 format
                        let row_size = format.width as usize * bytes_per_pixel;
                        let stride = format.stride as usize;

                        let mut rgb_data = Vec::with_capacity(format.height as usize * row_size);

                        for row in 0..format.height as usize {
                            let row_start = row * stride;
                            let row_end = row_start + row_size;
                            rgb_data.extend_from_slice(&buffer[row_start..row_end]);
                        }

                        yield Frame {
                            data: rgb_data,
                            height: format.height,
                            width: format.width,
                        }
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
