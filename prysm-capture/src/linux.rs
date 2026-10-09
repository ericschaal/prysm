use crate::{Frame, PixelFormat, PrysmCapturer, YuvRange, YuvStandardMatrix};
use anyhow::{Context, Result};
use futures::Stream;
use tokio_util::sync::CancellationToken;
use v4l::buffer::Type;
use v4l::io::traits::{CaptureStream, Stream as _};
use v4l::prelude::MmapStream;
use v4l::video::Capture;
use v4l::{Device, Format, FourCC};

#[allow(non_upper_case_globals)] // Names come from the generated kernel bindings.
fn yuv_colorimetry(
    colorspace: u32,
    encoding: u32,
    quantization: u32,
) -> Result<(YuvRange, YuvStandardMatrix)> {
    use v4l::v4l_sys::*;
    let encoding = if encoding == v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_DEFAULT {
        match colorspace {
            v4l2_colorspace_V4L2_COLORSPACE_REC709 | v4l2_colorspace_V4L2_COLORSPACE_DCI_P3 => {
                v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_709
            }
            v4l2_colorspace_V4L2_COLORSPACE_SMPTE240M => {
                v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_SMPTE240M
            }
            v4l2_colorspace_V4L2_COLORSPACE_BT2020 => v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_BT2020,
            _ => v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_601,
        }
    } else {
        encoding
    };
    let matrix = match encoding {
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_601 | v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_SYCC => {
            YuvStandardMatrix::Bt601
        }
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_709 => YuvStandardMatrix::Bt709,
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_BT2020 => YuvStandardMatrix::Bt2020,
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_SMPTE240M => YuvStandardMatrix::Smpte240,
        _ => anyhow::bail!("Unsupported YUYV encoding: {encoding}"),
    };
    let range = match quantization {
        v4l2_quantization_V4L2_QUANTIZATION_FULL_RANGE => YuvRange::Full,
        v4l2_quantization_V4L2_QUANTIZATION_LIM_RANGE => YuvRange::Limited,
        v4l2_quantization_V4L2_QUANTIZATION_DEFAULT => {
            if colorspace == v4l2_colorspace_V4L2_COLORSPACE_JPEG {
                YuvRange::Full
            } else {
                YuvRange::Limited
            }
        }
        _ => anyhow::bail!("Unsupported YUYV quantization: {quantization}"),
    };
    Ok((range, matrix))
}

fn negotiated_colorimetry(device: &Device) -> Result<(YuvRange, YuvStandardMatrix)> {
    // v4l::Format omits ycbcr_enc, so read the complete negotiated kernel format.
    // SAFETY: G_FMT writes an initialized VideoCapture structure for a live device fd.
    let format = unsafe {
        let mut format = v4l::v4l_sys::v4l2_format {
            type_: Type::VideoCapture as u32,
            ..std::mem::zeroed()
        };
        v4l::v4l2::ioctl(
            device.handle().fd(),
            v4l::v4l2::vidioc::VIDIOC_G_FMT,
            std::ptr::from_mut(&mut format).cast(),
        )?;
        format.fmt.pix
    };
    // SAFETY: ycbcr_enc is the active union member for a YUYV capture format.
    yuv_colorimetry(
        format.colorspace,
        unsafe { format.__bindgen_anon_1.ycbcr_enc },
        format.quantization,
    )
}

pub struct V4lCapturer {
    device_path: String,
    shutdown_token: CancellationToken,
}

impl V4lCapturer {
    /// Opens a capture device. `device_path` selects a v4l device node;
    /// `None` defaults to `/dev/video0`.
    pub fn new(device_path: Option<&str>, shutdown_token: CancellationToken) -> Result<Self> {
        Ok(Self {
            device_path: device_path.unwrap_or("/dev/video0").to_string(),
            shutdown_token,
        })
    }

    fn create_stream(
        device: &mut Device,
        width: u32,
        height: u32,
    ) -> Result<(MmapStream<'_>, Format, YuvRange, YuvStandardMatrix)> {
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
                    if format.fourcc == FourCC::new(b"YUYV") && !format.width.is_multiple_of(2) {
                        continue;
                    }
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

                        let (range, matrix) = if format.fourcc == FourCC::new(b"YUYV") {
                            negotiated_colorimetry(device)?
                        } else {
                            (YuvRange::Full, YuvStandardMatrix::Bt601)
                        };
                        let mmap_stream = MmapStream::with_buffers(device, Type::VideoCapture, 4)
                            .context("Failed to create stream")?;

                        return Ok((mmap_stream, format, range, matrix));
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

            let (mut input_stream, format, range, matrix) =
                match Self::create_stream(&mut device, width, height) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to create stream: {}", e);
                        return;
                    }
                };
            // Bound the wait so a stalled camera cannot prevent cancellation.
            input_stream.set_timeout(std::time::Duration::from_secs(1));

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
                if self.shutdown_token.is_cancelled() || tx.is_closed() {
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

                        let mut frame =
                            Frame::new(frame_data, format.width, format.height, pixel_format);
                        frame.yuv_range = range;
                        frame.yuv_matrix = matrix;

                        let sent = futures::executor::block_on(crate::send_frame(
                            &tx,
                            frame,
                            &self.shutdown_token,
                        ));
                        if !sent {
                            tracing::info!(
                                "Capture cancelled or receiver dropped, stopping capture"
                            );
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                        // next() queues a buffer before waiting; restart to avoid queueing it twice.
                        if let Err(e) = input_stream.stop() {
                            tracing::error!("Failed to stop stalled capture stream: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use v4l::v4l_sys::*;

    #[test]
    fn default_and_explicit_color_metadata() {
        assert_eq!(
            yuv_colorimetry(v4l2_colorspace_V4L2_COLORSPACE_SRGB, 0, 0).unwrap(),
            (YuvRange::Limited, YuvStandardMatrix::Bt601)
        );
        assert_eq!(
            yuv_colorimetry(v4l2_colorspace_V4L2_COLORSPACE_REC709, 0, 0).unwrap(),
            (YuvRange::Limited, YuvStandardMatrix::Bt709)
        );
        assert_eq!(
            yuv_colorimetry(v4l2_colorspace_V4L2_COLORSPACE_JPEG, 0, 0).unwrap(),
            (YuvRange::Full, YuvStandardMatrix::Bt601)
        );
        assert_eq!(
            yuv_colorimetry(
                v4l2_colorspace_V4L2_COLORSPACE_REC709,
                v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_601,
                v4l2_quantization_V4L2_QUANTIZATION_FULL_RANGE
            )
            .unwrap(),
            (YuvRange::Full, YuvStandardMatrix::Bt601)
        );
        assert!(
            yuv_colorimetry(0, v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_BT2020_CONST_LUM, 0).is_err()
        );
    }
}
