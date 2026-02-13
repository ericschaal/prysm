use anyhow::{Context, Result};
use futures::Stream;
use prysm_capture::{Frame, PixelFormat, PrysmCapturer};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio_util::sync::CancellationToken;
use v4l::buffer::Type;
use v4l::io::traits::CaptureStream;
use v4l::prelude::MmapStream;
use v4l::video::Capture;
use v4l::{Device, Format, FourCC};

pub struct V4lCapturer {
    device_path: String,
    shutdown_token: CancellationToken,
}

impl V4lCapturer {
    pub fn new(device_path: &str, shutdown_token: CancellationToken) -> Result<Self> {
        Ok(Self {
            device_path: device_path.to_string(),
            shutdown_token,
        })
    }

    fn create_stream(
        device: &mut Device,
        width: u32,
        height: u32,
    ) -> Result<(MmapStream<'_>, Format)> {
        let mut fmt = device.format()?;

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
            match device.set_format(&fmt) {
                Ok(_) => {
                    let format = device.format()?;

                    // Validate that a supported format was set
                    if format.fourcc == FourCC::new(b"YUYV")
                        || format.fourcc == FourCC::new(b"RGB3")
                        || format.fourcc == FourCC::new(b"BGR3")
                    {
                        tracing::info!(
                            "Video format set to: {:?} {}x{} (stride: {})",
                            format.fourcc,
                            format.width,
                            format.height,
                            format.stride
                        );

                        let mmap_stream = MmapStream::with_buffers(device, Type::VideoCapture, 4)
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
    fn into_stream(self, width: u32, height: u32) -> impl Stream<Item = Frame> + Send + 'static {
        use tokio_stream::wrappers::ReceiverStream;

        // Create channel for sending frames from blocking thread to async
        let (tx, rx) = tokio::sync::mpsc::channel(4);

        // Create atomic shutdown flag for OS thread (can't use async CancellationToken in blocking context)
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let shutdown_flag_clone = shutdown_flag.clone();
        let shutdown_token = self.shutdown_token.clone();

        // Spawn async task to bridge cancellation token to atomic flag
        tokio::spawn(async move {
            shutdown_token.cancelled().await;
            shutdown_flag_clone.store(true, Ordering::Relaxed);
        });

        // Spawn OS thread for blocking v4l I/O
        std::thread::spawn(move || {
            tracing::info!("Opening video device: {}", self.device_path);
            let mut device = match Device::with_path(&self.device_path) {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("Failed to open video device: {}", e);
                    return;
                }
            };

            let (mut input_stream, format) = match Self::create_stream(&mut device, width, height) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to create stream: {}", e);
                    return;
                }
            };

            // Determine format
            let (pixel_format, bytes_per_pixel) = match format.fourcc.str() {
                Ok("YUYV") => (PixelFormat::YUYV, 2),
                Ok("RGB3") => (PixelFormat::RGB24, 3),
                Ok("BGR3") => (PixelFormat::BGR24, 3),
                _ => {
                    tracing::error!("Unsupported format: {:?}", format.fourcc);
                    return;
                }
            };

            tracing::info!("Stream started with format: {:?}", pixel_format);

            // Blocking loop (appropriate for blocking I/O)
            loop {
                // Check shutdown flag before attempting to read
                if shutdown_flag.load(Ordering::Relaxed) {
                    tracing::info!("Shutdown signal received, stopping v4l capture");
                    break;
                }

                match input_stream.next() {
                    Ok((buffer, _metadata)) => {
                        // Extract frame data (same as current code)
                        let row_size = format.width as usize * bytes_per_pixel;
                        let stride = format.stride as usize;
                        let mut frame_data = Vec::with_capacity(format.height as usize * row_size);

                        for row in 0..format.height as usize {
                            let row_start = row * stride;
                            let row_end = row_start + row_size;
                            frame_data.extend_from_slice(&buffer[row_start..row_end]);
                        }

                        let frame =
                            Frame::new(frame_data, format.width, format.height, pixel_format);

                        if tx.blocking_send(frame).is_err() {
                            tracing::info!("Frame receiver dropped, stopping capture");
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error capturing frame: {}", e);
                        break;
                    }
                }
            }
        });

        // Return async stream backed by channel
        ReceiverStream::new(rx)
    }
}
