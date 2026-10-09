use std::slice;
use std::sync::Mutex;
use std::sync::mpsc as std_mpsc;

use anyhow::{Context, Result, anyhow};
use av_foundation::capture_device::AVCaptureDevice;
use av_foundation::capture_input::AVCaptureDeviceInput;
use av_foundation::capture_output_base::AVCaptureOutput;
use av_foundation::capture_session::{AVCaptureConnection, AVCaptureSession};
use av_foundation::capture_video_data_output::{
    AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate,
};
use av_foundation::media_format::AVMediaTypeVideo;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_video::buffer::TCVBuffer;
use core_video::image_buffer::{CVImageBufferKeys, ycbcr_matrix_get_integer_code_point_for_string};
use core_video::pixel_buffer::{
    CVPixelBuffer, kCVPixelBufferHeightKey, kCVPixelBufferLock_ReadOnly,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelFormatType_422YpCbCr8_yuvs,
    kCVPixelFormatType_422YpCbCr8FullRange,
};
use core_video::r#return::kCVReturnSuccess;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use futures::Stream;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSString};
use tokio_util::sync::CancellationToken;

use crate::{Frame, PixelFormat, PrysmCapturer, YuvRange, YuvStandardMatrix};

struct DelegateIvars {
    sender: Mutex<Option<tokio::sync::mpsc::Sender<Frame>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "PrysmCaptureSampleBufferDelegate"]
    #[ivars = DelegateIvars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Delegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn capture_output_did_output_sample_buffer(
            &self,
            _capture_output: &AVCaptureOutput,
            sample_buffer: CMSampleBufferRef,
            _connection: &AVCaptureConnection,
        ) {
            let sample_buffer = unsafe { CMSampleBuffer::wrap_under_get_rule(sample_buffer) };
            let Some(image_buffer) = sample_buffer.get_image_buffer() else {
                return;
            };
            let Some(pixel_buffer) = image_buffer.downcast::<CVPixelBuffer>() else {
                return;
            };
            let Some(frame) = frame_from_pixel_buffer(&pixel_buffer) else {
                return;
            };
            if let Ok(sender) = self.ivars().sender.lock()
                && let Some(sender) = sender.as_ref()
            {
                // Drop the frame if the consumer is falling behind.
                let _ = sender.try_send(frame);
            }
        }
    }

    impl Delegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Option<Retained<Self>> {
            let this = this.set_ivars(DelegateIvars {
                sender: Mutex::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }
    }
);

impl Delegate {
    fn new(sender: tokio::sync::mpsc::Sender<Frame>) -> Retained<Self> {
        let this: Retained<Self> = unsafe { msg_send![Self::alloc(), init] };
        *this.ivars().sender.lock().expect("sender mutex poisoned") = Some(sender);
        this
    }
}

/// Copies a `yuvs` (YUYV) pixel buffer into a [`Frame`], stripping any row
/// padding so the data matches the tightly-packed layout produced by the v4l
/// capturer on Linux.
fn frame_from_pixel_buffer(pixel_buffer: &CVPixelBuffer) -> Option<Frame> {
    let range = if pixel_buffer.get_pixel_format() == kCVPixelFormatType_422YpCbCr8_yuvs {
        YuvRange::Limited
    } else if pixel_buffer.get_pixel_format() == kCVPixelFormatType_422YpCbCr8FullRange {
        YuvRange::Full
    } else {
        tracing::error!(
            "Unexpected pixel format: {:?}",
            pixel_buffer.get_pixel_format().to_be_bytes()
        );
        return None;
    };
    if !pixel_buffer.get_width().is_multiple_of(2) {
        tracing::error!("YUYV width must be even");
        return None;
    }
    let matrix = pixel_buffer
        .as_buffer()
        .get_attachment(&CVImageBufferKeys::YCbCrMatrix.into(), None)
        .and_then(|value| value.downcast::<CFString>())
        .map_or(6, |value| {
            ycbcr_matrix_get_integer_code_point_for_string(&value)
        });
    let matrix = match matrix {
        1 => YuvStandardMatrix::Bt709,
        5 | 6 => YuvStandardMatrix::Bt601,
        7 => YuvStandardMatrix::Smpte240,
        9 => YuvStandardMatrix::Bt2020,
        _ => {
            tracing::error!("Unsupported YUYV color matrix: {matrix}");
            return None;
        }
    };
    if pixel_buffer.lock_base_address(kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess {
        return None;
    }

    let width = pixel_buffer.get_width();
    let height = pixel_buffer.get_height();
    let stride = pixel_buffer.get_bytes_per_row();
    let base = unsafe { pixel_buffer.get_base_address() }
        .cast_const()
        .cast::<u8>();

    let frame = if base.is_null() {
        None
    } else {
        let row_size = width * 2; // YUYV: 2 bytes per pixel
        let data = if stride == row_size {
            // No row padding: copy the whole plane in one memcpy
            unsafe { slice::from_raw_parts(base, height * row_size) }.to_vec()
        } else {
            let mut data = Vec::with_capacity(height * row_size);
            for row in 0..height {
                let row = unsafe { slice::from_raw_parts(base.add(row * stride), row_size) };
                data.extend_from_slice(row);
            }
            data
        };
        match (u32::try_from(width), u32::try_from(height)) {
            (Ok(width), Ok(height)) => {
                let mut frame = Frame::new(data, width, height, PixelFormat::YUYV);
                frame.yuv_range = range;
                frame.yuv_matrix = matrix;
                Some(frame)
            }
            _ => None,
        }
    };

    pixel_buffer.unlock_base_address(kCVPixelBufferLock_ReadOnly);
    frame
}

fn video_settings(width: u32, height: u32) -> Retained<NSDictionary<NSString, NSObject>> {
    // The kCVPixelBuffer* keys are CFStrings, toll-free bridged to NSString.
    let pixel_format_key = unsafe { &*kCVPixelBufferPixelFormatTypeKey.cast::<NSString>() };
    let width_key = unsafe { &*kCVPixelBufferWidthKey.cast::<NSString>() };
    let height_key = unsafe { &*kCVPixelBufferHeightKey.cast::<NSString>() };

    fn ns_object(number: Retained<NSNumber>) -> Retained<NSObject> {
        Retained::into_super(Retained::into_super(number))
    }

    NSDictionary::from_retained_objects(
        &[pixel_format_key, width_key, height_key],
        &[
            ns_object(NSNumber::new_u32(kCVPixelFormatType_422YpCbCr8_yuvs)),
            ns_object(NSNumber::new_u32(width)),
            ns_object(NSNumber::new_u32(height)),
        ],
    )
}

/// Keeps the `AVFoundation` objects alive for as long as frames are flowing.
struct RunningSession {
    session: Retained<AVCaptureSession>,
    _output: Retained<AVCaptureVideoDataOutput>,
    _delegate: Retained<Delegate>,
    _queue: DispatchRetained<DispatchQueue>,
}

impl Drop for RunningSession {
    fn drop(&mut self) {
        self.session.stop_running();
    }
}

fn start_session(
    device_uid: Option<&str>,
    sender: tokio::sync::mpsc::Sender<Frame>,
    width: u32,
    height: u32,
) -> Result<RunningSession> {
    let device = match device_uid {
        Some(uid) => AVCaptureDevice::device_with_unique_id(&NSString::from_str(uid)),
        None => AVCaptureDevice::default_device_with_media_type(unsafe { AVMediaTypeVideo }),
    }
    .context("No video capture device found")?;

    tracing::info!("Opening video device: {}", device.localized_name());

    let session = AVCaptureSession::new();
    let input = AVCaptureDeviceInput::from_device(&device)
        .map_err(|e| anyhow!("Failed to open device input: {e:?}"))?;
    let output = AVCaptureVideoDataOutput::new();
    let delegate = Delegate::new(sender);
    let queue = DispatchQueue::new("prysm.capture.video", DispatchQueueAttr::SERIAL);

    output.set_sample_buffer_delegate(ProtocolObject::from_ref(&*delegate), &queue);
    output.set_always_discards_late_video_frames(true);

    session.begin_configuration();
    if !session.can_add_input(&input) {
        session.commit_configuration();
        return Err(anyhow!("Cannot add capture input to session"));
    }
    session.add_input(&input);
    if !session.can_add_output(&output) {
        session.commit_configuration();
        return Err(anyhow!("Cannot add capture output to session"));
    }
    session.add_output(&output);
    output.set_video_settings(&video_settings(width, height));
    session.commit_configuration();

    session.start_running();
    tracing::info!("Video format set to: YUYV {width}x{height}");

    Ok(RunningSession {
        session,
        _output: output,
        _delegate: delegate,
        _queue: queue,
    })
}

pub struct AVFoundationCapturer {
    device_uid: Option<String>,
    shutdown_token: CancellationToken,
}

async fn stop_when_capture_closes(
    sender: tokio::sync::mpsc::Sender<Frame>,
    shutdown: CancellationToken,
    stop: std_mpsc::Sender<()>,
) {
    tokio::select! {
        () = shutdown.cancelled() => {},
        () = sender.closed() => {},
    }
    let _ = stop.send(());
}

impl AVFoundationCapturer {
    /// Opens a capture device. `device_uid` selects a device by its
    /// `AVFoundation` unique ID; `None` picks the system default video device.
    pub fn new(device_uid: Option<&str>, shutdown_token: CancellationToken) -> Result<Self> {
        Ok(Self {
            device_uid: device_uid.map(str::to_string),
            shutdown_token,
        })
    }
}

impl PrysmCapturer for AVFoundationCapturer {
    fn into_stream(self, width: u32, height: u32) -> impl Stream<Item = Frame> + Send + 'static {
        use tokio_stream::wrappers::ReceiverStream;

        // Frames flow delegate queue -> channel -> async stream. The capture
        // session itself is owned by a dedicated OS thread because the
        // AVFoundation objects are not Send.
        let (tx, rx) = tokio::sync::mpsc::channel(4);

        // Bridge the async cancellation token to the blocking thread.
        let (stop_tx, stop_rx) = std_mpsc::channel::<()>();
        let runtime = tokio::runtime::Handle::current();

        std::thread::spawn(move || {
            let session = match start_session(self.device_uid.as_deref(), tx.clone(), width, height)
            {
                Ok(session) => session,
                Err(e) => {
                    tracing::error!("Failed to start capture session: {e}");
                    return;
                }
            };

            runtime.spawn(stop_when_capture_closes(tx, self.shutdown_token, stop_tx));

            // Block until shutdown is signalled (or the bridge task is gone).
            let _ = stop_rx.recv();
            tracing::info!("Shutdown signal received, stopping AVFoundation capture");
            drop(session);
        });

        ReceiverStream::new(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn failed_startup_closes_frame_stream() {
        use futures::StreamExt;
        let shutdown = CancellationToken::new();
        let capturer =
            AVFoundationCapturer::new(Some("prysm-test-missing-device"), shutdown.clone()).unwrap();
        let mut stream = capturer.into_stream(640, 360);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                .await
                .unwrap()
                .is_none()
        );
        assert!(!shutdown.is_cancelled());
    }

    #[tokio::test]
    async fn capture_stops_on_cancellation_or_consumer_drop() {
        for cancel in [false, true] {
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            let shutdown = CancellationToken::new();
            let (stop_tx, stop_rx) = std_mpsc::channel();
            let stop = stop_when_capture_closes(sender, shutdown.clone(), stop_tx);
            futures::pin_mut!(stop);
            assert!(futures::poll!(&mut stop).is_pending());
            if cancel {
                shutdown.cancel();
            } else {
                drop(receiver);
            }
            stop.await;
            assert!(stop_rx.try_recv().is_ok());
        }
    }

    #[test]
    fn pixel_buffer_metadata_and_row_padding_reach_frame() {
        let buffer = CVPixelBuffer::new(kCVPixelFormatType_422YpCbCr8_yuvs, 4, 2, None).unwrap();
        let matrix = core_video::image_buffer::CVImageBufferYCbCrMatrix::ITU_R_709_2;
        let matrix: CFString = matrix.into();
        buffer.as_buffer().set_attachment(
            &CVImageBufferKeys::YCbCrMatrix.into(),
            &matrix.as_CFType(),
            core_video::buffer::kCVAttachmentMode_ShouldPropagate,
        );
        assert_eq!(buffer.lock_base_address(0), kCVReturnSuccess);
        let stride = buffer.get_bytes_per_row();
        // SAFETY: The buffer is locked and each row has space for four YUYV pixels.
        unsafe {
            let base = buffer.get_base_address().cast::<u8>();
            for row in 0..2 {
                std::ptr::copy_nonoverlapping(
                    [16, 128, 235, 128, 16, 128, 235, 128].as_ptr(),
                    base.add(row * stride),
                    8,
                );
            }
        }
        buffer.unlock_base_address(0);
        let frame = frame_from_pixel_buffer(&buffer).unwrap();
        assert_eq!(
            frame.as_slice(),
            [16, 128, 235, 128, 16, 128, 235, 128].repeat(2)
        );
        assert_eq!(frame.yuv_range, YuvRange::Limited);
        assert_eq!(frame.yuv_matrix, YuvStandardMatrix::Bt709);
    }
}
