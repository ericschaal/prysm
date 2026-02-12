use anyhow::{Context, Result};
use async_stream::stream;
use futures::{Stream};
use prysm_capture::{CaptureMessage, Frame, Info, PrysmCapturer};
use tracing::info;
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
    fn start(&mut self, width: u32, height: u32) -> impl Stream<Item = CaptureMessage> + '_ {
        let (mut input_stream, format) = self.create_stream(width, height).expect("Failed to create stream");
        info!("stream started with format: {:?}", format);

        stream! {

            yield CaptureMessage::Info(Info {height: format.height, width: format.width});

            loop {
                match input_stream.next() {
                    Ok((buffer, _metadata)) => {
                        yield CaptureMessage::Frame(Frame(buffer.to_vec()));
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
