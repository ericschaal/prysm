use std::num::NonZeroU32;
use std::slice;
use std::sync::Mutex;

use anyhow::{Context, anyhow};
use av_foundation::capture_device::{AVCaptureDevice, AVCaptureDeviceWasDisconnectedNotification};
use av_foundation::capture_input::AVCaptureDeviceInput;
use av_foundation::capture_output_base::AVCaptureOutput;
use av_foundation::capture_session::{
    AVCaptureConnection, AVCaptureSession, AVCaptureSessionErrorKey,
    AVCaptureSessionRuntimeErrorNotification,
};
use av_foundation::capture_video_data_output::{
    AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate,
};
use av_foundation::media_format::AVMediaTypeVideo;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_media::format_description::CMVideoFormatDescription;
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_media::time::CMTime;
use core_video::buffer::TCVBuffer;
use core_video::image_buffer::{CVImageBufferKeys, ycbcr_matrix_get_integer_code_point_for_string};
use core_video::pixel_buffer::{
    CVPixelBuffer, kCVPixelBufferHeightKey, kCVPixelBufferLock_ReadOnly,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelFormatType_422YpCbCr8_yuvs,
    kCVPixelFormatType_422YpCbCr8FullRange,
};
use core_video::r#return::kCVReturnSuccess;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_foundation::{
    NSDictionary, NSError, NSNotification, NSNotificationCenter, NSNumber, NSObject,
    NSObjectProtocol, NSString,
};
use tokio_util::sync::CancellationToken;

use crate::{CaptureError, Frame, FramePool, PixelFormat, Result, YuvRange, YuvStandardMatrix};

struct CaptureState {
    sender: tokio::sync::watch::Sender<Option<Frame>>,
    pool: FramePool,
    stats: prysm_metrics::TraceStats,
}

struct DelegateIvars {
    capture: Mutex<Option<CaptureState>>,
    failure: CancellationToken,
    error: Mutex<Option<CaptureError>>,
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
            if let Ok(mut capture) = self.ivars().capture.lock()
                && let Some(capture) = capture.as_mut()
            {
                let started = capture.stats.start();
                if let Some(frame) = frame_from_pixel_buffer(&pixel_buffer, &mut capture.pool) {
                    capture.stats.record(started, frame.len(), Some(capture.pool.stats()));
                    let _ = capture.sender.send(Some(frame));
                } else {
                    capture.stats.failure();
                }
            }
        }
    }

    impl Delegate {
        #[unsafe(method(captureFailed:))]
        fn capture_failed(&self, notification: &NSNotification) {
            let error = format!("Capture failed: {} ({:?})", notification.name(), notification.userInfo());
            // SAFETY: This notification name is a process-lifetime Foundation string.
            let error = if &*notification.name() == unsafe { AVCaptureDeviceWasDisconnectedNotification } {
                CaptureError::Disconnected { details: error }
            } else {
                // SAFETY: This dictionary key is a process-lifetime Foundation string.
                let source = notification.userInfo()
                    .and_then(|info| info.objectForKey(unsafe { AVCaptureSessionErrorKey }))
                    .and_then(|value| value.downcast::<NSError>().ok());
                CaptureError::Backend(match source {
                    Some(source) => anyhow::Error::new(source).context(error),
                    None => anyhow!(error),
                })
            };
            *self.ivars().error.lock().expect("capture error mutex poisoned") = Some(error);
            self.ivars().failure.cancel();
        }

        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Option<Retained<Self>> {
            let this = this.set_ivars(DelegateIvars {
                capture: Mutex::new(None),
                failure: CancellationToken::new(),
                error: Mutex::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }
    }
);

impl Delegate {
    fn new(sender: tokio::sync::watch::Sender<Option<Frame>>) -> Retained<Self> {
        let this: Retained<Self> = unsafe { msg_send![Self::alloc(), init] };
        *this.ivars().capture.lock().expect("capture mutex poisoned") = Some(CaptureState {
            sender,
            pool: FramePool::default(),
            stats: prysm_metrics::TraceStats::new("avfoundation_copy"),
        });
        this
    }

    fn observe_failure(&self, name: &NSString, object: &AnyObject) {
        // SAFETY: captureFailed: accepts NSNotification; this observer is removed on drop.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(captureFailed:),
                Some(name),
                Some(object),
            );
        }
    }
}

impl Drop for Delegate {
    fn drop(&mut self) {
        // SAFETY: self is the live observer registered by observe_failure.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self) };
    }
}

/// Copies a `yuvs` (YUYV) pixel buffer into a [`Frame`], stripping any row
/// padding so the data matches the tightly-packed layout produced by the v4l
/// capturer on Linux.
fn frame_from_pixel_buffer(pixel_buffer: &CVPixelBuffer, pool: &mut FramePool) -> Option<Frame> {
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
        (|| {
            let row_size = width.checked_mul(2)?;
            let size = stride
                .checked_mul(height.checked_sub(1)?)?
                .checked_add(row_size)?;
            if row_size == 0 || stride < row_size || size > isize::MAX as usize {
                return None;
            }
            let mut frame = pool
                .acquire(
                    u32::try_from(width).ok()?,
                    u32::try_from(height).ok()?,
                    PixelFormat::YUYV { range, matrix },
                )
                .ok()?;
            if stride == row_size {
                // SAFETY: the locked packed plane contains exactly these pixel bytes.
                frame
                    .as_mut_slice()
                    .copy_from_slice(unsafe { slice::from_raw_parts(base, size) });
            } else {
                for (row, target) in frame.as_mut_slice().chunks_exact_mut(row_size).enumerate() {
                    // SAFETY: row bounds were checked above; skip native padding bytes.
                    let pixels = unsafe { slice::from_raw_parts(base.add(row * stride), row_size) };
                    target.copy_from_slice(pixels);
                }
            }
            Some(frame.finish())
        })()
    };

    pixel_buffer.unlock_base_address(kCVPixelBufferLock_ReadOnly);
    frame
}

fn video_settings(width: u32, height: u32) -> Retained<NSDictionary<NSString, NSObject>> {
    fn ns_object(number: Retained<NSNumber>) -> Retained<NSObject> {
        Retained::into_super(Retained::into_super(number))
    }

    // The kCVPixelBuffer* keys are CFStrings, toll-free bridged to NSString.
    let pixel_format_key = unsafe { &*kCVPixelBufferPixelFormatTypeKey.cast::<NSString>() };
    let width_key = unsafe { &*kCVPixelBufferWidthKey.cast::<NSString>() };
    let height_key = unsafe { &*kCVPixelBufferHeightKey.cast::<NSString>() };

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
    output: Retained<AVCaptureVideoDataOutput>,
    delegate: Retained<Delegate>,
    queue: DispatchRetained<DispatchQueue>,
}

impl Drop for RunningSession {
    fn drop(&mut self) {
        // Publication ends before stop; the serial barrier settles every queued callback.
        self.delegate
            .ivars()
            .capture
            .lock()
            .expect("capture mutex poisoned")
            .take();
        self.session.stop_running();
        // SAFETY: clearing the native delegate prevents future callback scheduling.
        unsafe {
            let _: () = msg_send![&*self.output, setSampleBufferDelegate: std::ptr::null::<AnyObject>(), queue: std::ptr::null::<AnyObject>()];
        }
        self.queue.exec_sync(|| {});
    }
}

fn configure_capture(
    device: &AVCaptureDevice,
    width: u32,
    height: u32,
    refresh_rate_hz: NonZeroU32,
) -> anyhow::Result<()> {
    let timescale = i32::try_from(refresh_rate_hz.get())
        .context("capture rate exceeds AVFoundation's timescale")?;
    let requested_width =
        i32::try_from(width).context("capture width exceeds AVFoundation's dimensions")?;
    let requested_height =
        i32::try_from(height).context("capture height exceeds AVFoundation's dimensions")?;
    let formats = device.formats();
    let format = formats
        .iter()
        .find(|format| {
            let Some(description) = format
                .format_description()
                .downcast::<CMVideoFormatDescription>()
            else {
                return false;
            };
            let dimensions = description.get_dimensions();
            dimensions.width == requested_width
                && dimensions.height == requested_height
                && format
                    .video_supported_frame_rate_ranges()
                    .iter()
                    .any(|range| {
                        range.min_frame_rate() <= f64::from(refresh_rate_hz.get())
                            && f64::from(refresh_rate_hz.get()) <= range.max_frame_rate()
                    })
        })
        .with_context(|| {
            format!("capture device does not support {width}x{height} at {refresh_rate_hz} Hz")
        })?;
    device
        .lock_for_configuration()
        .context("cannot lock capture device to set format and frame rate")?;
    device.set_active_format(&format);
    let duration = CMTime::make(1, timescale);
    device.set_active_video_min_frame_duration(duration);
    device.set_active_video_max_frame_duration(duration);
    device.unlock_for_configuration();
    Ok(())
}

fn start_session(
    device_uid: Option<&str>,
    sender: tokio::sync::watch::Sender<Option<Frame>>,
    width: u32,
    height: u32,
    refresh_rate_hz: NonZeroU32,
    shutdown: &CancellationToken,
) -> Result<Option<RunningSession>> {
    if shutdown.is_cancelled() {
        return Ok(None);
    }
    let device = match device_uid {
        Some(uid) => AVCaptureDevice::device_with_unique_id(&NSString::from_str(uid)),
        None => AVCaptureDevice::default_device_with_media_type(unsafe { AVMediaTypeVideo }),
    }
    .ok_or_else(|| CaptureError::DeviceNotFound {
        device: device_uid.map(str::to_owned),
        source: None,
    })?;

    tracing::info!("Opening video device: {}", device.localized_name());

    let session = AVCaptureSession::new();
    let input =
        AVCaptureDeviceInput::from_device(&device).context("failed to open device input")?;
    let output = AVCaptureVideoDataOutput::new();
    let delegate = Delegate::new(sender);
    let queue = DispatchQueue::new("prysm.capture.video", DispatchQueueAttr::SERIAL);

    // SAFETY: These notification names are process-lifetime Foundation strings.
    unsafe {
        delegate.observe_failure(AVCaptureSessionRuntimeErrorNotification, &session);
        delegate.observe_failure(AVCaptureDeviceWasDisconnectedNotification, &device);
    }

    output.set_sample_buffer_delegate(ProtocolObject::from_ref(&*delegate), &queue);
    output.set_always_discards_late_video_frames(true);

    session.begin_configuration();
    if !session.can_add_input(&input) {
        session.commit_configuration();
        return Err(anyhow!("cannot add capture input to session").into());
    }
    session.add_input(&input);
    if !session.can_add_output(&output) {
        session.commit_configuration();
        return Err(anyhow!("cannot add capture output to session").into());
    }
    session.add_output(&output);
    output.set_video_settings(&video_settings(width, height));
    let configured = configure_capture(&device, width, height, refresh_rate_hz);
    session.commit_configuration();
    configured?;

    let running = RunningSession {
        session,
        output,
        delegate,
        queue,
    };
    if shutdown.is_cancelled() {
        return Ok(None);
    }
    running.session.start_running();
    let actual = device
        .get_active_format()
        .format_description()
        .downcast::<CMVideoFormatDescription>()
        .context("capture device did not retain a video format")?
        .get_dimensions();
    if i64::from(actual.width) != i64::from(width) || i64::from(actual.height) != i64::from(height)
    {
        return Err(anyhow!(
            "capture device changed resolution to {}x{}; requested {width}x{height}",
            actual.width,
            actual.height
        )
        .into());
    }
    if let Some(error) = running
        .delegate
        .ivars()
        .error
        .lock()
        .expect("capture error mutex poisoned")
        .take()
    {
        return Err(error);
    }
    if !running.session.is_running() || running.delegate.ivars().failure.is_cancelled() {
        return Err(anyhow!("capture session failed to start").into());
    }
    tracing::info!("Video format set to: YUYV {width}x{height}");
    Ok(Some(running))
}

async fn stop_when_capture_closes(
    sender: tokio::sync::watch::Sender<Option<Frame>>,
    shutdown: CancellationToken,
    failure: CancellationToken,
) {
    tokio::select! {
        () = shutdown.cancelled() => {},
        () = sender.closed() => {},
        () = failure.cancelled() => {},
    }
}

pub(crate) fn list_devices() -> Vec<crate::DeviceInfo> {
    AVCaptureDevice::devices_with_media_type(unsafe { AVMediaTypeVideo })
        .iter()
        .map(|device| crate::DeviceInfo {
            name: device.localized_name().to_string(),
            id: device.unique_id().to_string(),
        })
        .collect()
}

pub(crate) fn run(
    device_uid: Option<&str>,
    width: u32,
    height: u32,
    rate: NonZeroU32,
    frames: tokio::sync::watch::Sender<Option<Frame>>,
    ready: tokio::sync::oneshot::Sender<()>,
    shutdown: CancellationToken,
) -> Result<()> {
    let Some(session) = start_session(device_uid, frames.clone(), width, height, rate, &shutdown)?
    else {
        return Ok(());
    };
    if shutdown.is_cancelled() {
        return Ok(());
    }
    let _ = ready.send(());
    // The native thread itself waits; there is no detached monitor or nested runtime.
    futures::executor::block_on(stop_when_capture_closes(
        frames,
        shutdown,
        session.delegate.ivars().failure.clone(),
    ));
    let error = session
        .delegate
        .ivars()
        .error
        .lock()
        .expect("capture error mutex poisoned")
        .take();
    drop(session);
    error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_notification_preserves_native_error() {
        let object = NSObject::new();
        let (sender, _receiver) = tokio::sync::watch::channel(None::<Frame>);
        let delegate = Delegate::new(sender);
        // SAFETY: Foundation keys/names are static; all notification objects are live.
        unsafe {
            delegate.observe_failure(AVCaptureSessionRuntimeErrorNotification, &object);
            let source =
                NSError::errorWithDomain_code_userInfo(&NSString::from_str("prysm-test"), 42, None);
            let info = NSDictionary::from_retained_objects(
                &[AVCaptureSessionErrorKey],
                &[Retained::into_super(source)],
            );
            NSNotificationCenter::defaultCenter().postNotificationName_object_userInfo(
                AVCaptureSessionRuntimeErrorNotification,
                Some(&object),
                Some(info.cast_unchecked()),
            );
        }
        let error = delegate.ivars().error.lock().unwrap().take().unwrap();
        let CaptureError::Backend(error) = error else {
            panic!("expected backend error")
        };
        assert_eq!(
            error.downcast_ref::<Retained<NSError>>().unwrap().code(),
            42
        );
    }

    #[tokio::test]
    async fn capture_failure_notifications_stop_capture_and_close_the_stream() {
        // SAFETY: These are process-lifetime notification names.
        let names = unsafe {
            [
                AVCaptureSessionRuntimeErrorNotification,
                AVCaptureDeviceWasDisconnectedNotification,
            ]
        };
        for name in names {
            let object = NSObject::new();
            let unrelated = NSObject::new();
            let (sender, mut receiver) = tokio::sync::watch::channel(None::<Frame>);
            let delegate = Delegate::new(sender.clone());
            delegate.observe_failure(name, &object);
            let failure = delegate.ivars().failure.clone();
            let bridge =
                stop_when_capture_closes(sender, CancellationToken::new(), failure.clone());
            futures::pin_mut!(bridge);
            assert!(futures::poll!(&mut bridge).is_pending());

            // SAFETY: These live NSObject instances identify the notification source;
            // the observer only reads the notification name, not its object.
            unsafe {
                NSNotificationCenter::defaultCenter()
                    .postNotificationName_object(name, Some(&unrelated));
                assert!(
                    !failure.is_cancelled(),
                    "another capture must not stop this one"
                );
                NSNotificationCenter::defaultCenter()
                    .postNotificationName_object(name, Some(&object));
            }
            tokio::time::timeout(std::time::Duration::from_secs(1), bridge)
                .await
                .unwrap();
            let error = delegate.ivars().error.lock().unwrap().take().unwrap();
            assert!(error.to_string().contains(&name.to_string()));
            assert_eq!(
                matches!(error, CaptureError::Disconnected { .. }),
                name == unsafe { AVCaptureDeviceWasDisconnectedNotification },
            );
            drop(delegate);
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(1), receiver.changed())
                    .await
                    .unwrap()
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn failed_startup_preserves_capture_error() {
        let mut capture = crate::DeviceCapture::open(
            Some("prysm-test-missing-device"),
            640,
            360,
            30.try_into().unwrap(),
        )
        .unwrap();
        assert!((&mut capture.ready).await.is_err());
        let exit = tokio::time::timeout(std::time::Duration::from_secs(5), capture.worker.join())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            exit.failure,
            Some(CaptureError::DeviceNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn capture_stops_on_cancellation_or_consumer_drop() {
        for cancel in [false, true] {
            let (sender, receiver) = tokio::sync::watch::channel(None::<Frame>);
            let shutdown = CancellationToken::new();
            let stop = stop_when_capture_closes(sender, shutdown.clone(), CancellationToken::new());
            futures::pin_mut!(stop);
            assert!(futures::poll!(&mut stop).is_pending());
            if cancel {
                shutdown.cancel();
            } else {
                drop(receiver);
            }
            stop.await;
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
        let frame = frame_from_pixel_buffer(&buffer, &mut FramePool::default()).unwrap();
        assert_eq!(
            frame.as_slice(),
            [16, 128, 235, 128, 16, 128, 235, 128].repeat(2)
        );
        assert_eq!(
            frame.format(),
            PixelFormat::YUYV {
                range: YuvRange::Limited,
                matrix: YuvStandardMatrix::Bt709
            }
        );
    }
}
