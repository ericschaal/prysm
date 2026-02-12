use anyhow::{Context, Result};
use async_stream::stream;
use futures::{Stream, stream};
use prysm_capture::VideoCaptureBackend;
use prysm_core::Frame;
use std::pin::Pin;
use std::sync::Arc;
use v4l::buffer::Type;
use v4l::io::traits::CaptureStream;
use v4l::prelude::MmapStream;
use v4l::video::Capture;
use v4l::{Device, FourCC};

pub struct VideoCapture {
    device: Device,
    width: u32,
    height: u32,
}

impl VideoCapture {
    pub fn new(device_path: &str, width: u32, height: u32) -> Result<Self> {
        tracing::info!("Opening video device: {}", device_path);
        let device = Device::with_path(device_path).context("Failed to open video device")?;

        Ok(Self {
            device,
            width,
            height,
        })
    }

    fn create_stream(&mut self) -> Result<MmapStream<'_>> {
        tracing::info!("Starting video capture at {}x{}", self.width, self.height);

        // Set format
        let mut fmt = self.device.format()?;
        tracing::info!("Original video device settings: {fmt:?}");

        fmt.width = self.width;
        fmt.height = self.height;
        fmt.fourcc = FourCC::new(b"RGB3"); // Try RGB24 first

        self.device
            .set_format(&fmt)
            .context("Format not supported")?;

        let fmt = self.device.format()?;
        tracing::info!(
            "Video format set to: {:?} {}x{}",
            fmt.fourcc,
            fmt.width,
            fmt.height
        );

        // Update actual dimensions
        self.width = fmt.width;
        self.height = fmt.height;

        // Create stream
        let mmap_stream = MmapStream::with_buffers(&self.device, Type::VideoCapture, 4)
            .context("Failed to create stream")?;

        Ok(mmap_stream)
    }
}

impl VideoCaptureBackend for VideoCapture {
    fn start(&mut self) -> impl Stream<Item = Frame> + '_ {
        let width = self.width;
        let height = self.height;
        let mut input = self.create_stream().expect("Failed to create stream");

        stream! {
            loop {
                match input.next() {
                    Ok((buffer, _metadata)) => {
                        yield Frame {
                            buffer: buffer.to_vec(),
                            width,
                            height,
                        };
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
