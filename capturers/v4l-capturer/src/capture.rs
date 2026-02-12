use std::sync::Arc;
use v4l::{Device, FourCC};
use v4l::prelude::MmapStream;
use anyhow::{Context, Result};
use futures::{stream, Stream};
use v4l::buffer::Type;
use v4l::io::traits::CaptureStream;
use v4l::video::Capture;
use prysm_capture::VideoCaptureBackend;

pub struct VideoCapture {
    device: Arc<Device>,
    width: u32,
    height: u32,
}

impl VideoCapture {
    pub fn new(device_path: &str, width: u32, height: u32) -> Result<Self> {
        tracing::info!("Opening video device: {}", device_path);
        let device = Device::with_path(device_path)
            .context("Failed to open video device")?;

        let device = Arc::new(device);

        Ok(Self {
            device,
            width,
            height,
        })
    }

    fn get_stream(&mut self) -> Result<MmapStream> {
        tracing::info!("Starting video capture at {}x{}", self.width, self.height);

        // Set format
        let mut fmt = self.device.format()?;
        fmt.width = self.width;
        fmt.height = self.height;
        fmt.fourcc = FourCC::new(b"RGB3"); // Try RGB24 first

        // // Try to set format, fallback to YUYV if RGB not available
        // if self.device.set_format(&fmt).is_err() {
        //     tracing::warn!("RGB3 format not supported, trying YUYV");
        //     fmt.fourcc = FourCC::new(b"YUYV");
        //     self.device.set_format(&fmt)
        //         .context("Failed to set video format")?;
        // }

        let fmt = self.device.format()?;
        tracing::info!("Video format set to: {:?} {}x{}",
                   fmt.fourcc, fmt.width, fmt.height);

        // Update actual dimensions
        self.width = fmt.width;
        self.height = fmt.height;

        // Create stream
        let stream = MmapStream::with_buffers(&self.device, Type::VideoCapture, 4)
            .context("Failed to create stream")?;

        Ok(stream)
    }
}

impl VideoCaptureBackend for VideoCapture {
    fn start(&mut self) -> impl Stream<Item=prysm_core::Frame> {
        let input = self.get_stream().expect("Failed to start video capture");

        stream::try_unfold(input, |mut input| async {
                match input.next() {
                    Ok((buffer, _metadata)) => {
                        let frame = prysm_core::Frame {buffer, width: self.width, height: self.height};
                        Ok(Some(frame))
                    }
                    Err(e) => {
                        Err(e)
                    },
                }
        })
    }
}