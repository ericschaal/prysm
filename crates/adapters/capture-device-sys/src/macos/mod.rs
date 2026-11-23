//! AVFoundation capture: a session whose sample buffers arrive on a serial
//! queue, while the calling thread waits for a stop or a failure.
mod access;
mod configure;
mod delegate;
mod pixel_buffer;

use std::num::NonZeroU32;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use av_foundation::capture_device::{AVCaptureDevice, AVCaptureDeviceWasDisconnectedNotification};
use av_foundation::capture_input::AVCaptureDeviceInput;
use av_foundation::capture_session::{AVCaptureSession, AVCaptureSessionRuntimeErrorNotification};
use av_foundation::capture_video_data_output::AVCaptureVideoDataOutput;
use av_foundation::media_format::AVMediaTypeVideo;
use core_media::format_description::CMVideoFormatDescription;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, msg_send};
use objc2_foundation::NSString;
use prysm_ports::Span;

use crate::{CaptureError, CaptureSink, Result};
use access::camera_access;
use configure::{configure_capture, video_settings};
use delegate::Delegate;

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
        // A lock poisoned by a callback's panic still holds the state to release.
        self.delegate
            .ivars()
            .capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        self.session.stop_running();
        // SAFETY: `setSampleBufferDelegate:queue:` exists on this output with
        // two nullable object arguments and no result; nil for both clears
        // the delegate, so no callback is scheduled after this.
        unsafe {
            let _: () = msg_send![&*self.output, setSampleBufferDelegate: std::ptr::null::<AnyObject>(), queue: std::ptr::null::<AnyObject>()];
        }
        self.queue.exec_sync(|| {});
    }
}

fn start_session(
    device_uid: Option<&str>,
    sink: &Arc<dyn CaptureSink>,
    width: u32,
    height: u32,
    refresh_rate_hz: NonZeroU32,
) -> Result<Option<RunningSession>> {
    if sink.should_stop(Duration::ZERO) {
        return Ok(None);
    }
    let device = match device_uid {
        Some(uid) => AVCaptureDevice::device_with_unique_id(&NSString::from_str(uid)),
        // SAFETY: a constant NSString AVFoundation defines for the process's life.
        None => AVCaptureDevice::default_device_with_media_type(unsafe { AVMediaTypeVideo }),
    }
    .ok_or_else(|| CaptureError::DeviceNotFound {
        device: device_uid.map(str::to_owned),
        source: None,
    })?;

    tracing::info!("Opening video device: {}", device.localized_name());
    // After the lookup, so a missing device never asks for access.
    if !camera_access(sink)? {
        return Ok(None);
    }

    let input =
        AVCaptureDeviceInput::from_device(&device).context("failed to open device input")?;
    // Built before the delegate is installed, so every return below, error
    // or not, takes the one teardown in its `Drop`; stopping a session that
    // never started does nothing.
    let running = RunningSession {
        session: AVCaptureSession::new(),
        output: AVCaptureVideoDataOutput::new(),
        delegate: Delegate::new(sink.clone(), refresh_rate_hz, Instant::now()),
        queue: DispatchQueue::new("prysm.capture.video", DispatchQueueAttr::SERIAL),
    };
    let RunningSession {
        session,
        output,
        delegate,
        queue,
    } = &running;

    // SAFETY: reads two notification names, constant NSStrings Foundation
    // defines for the process's life.
    let (runtime_error, disconnected) = unsafe {
        (
            AVCaptureSessionRuntimeErrorNotification,
            AVCaptureDeviceWasDisconnectedNotification,
        )
    };
    delegate.observe_failure(runtime_error, session);
    delegate.observe_failure(disconnected, &device);

    output.set_sample_buffer_delegate(ProtocolObject::from_ref(&**delegate), queue);
    output.set_always_discards_late_video_frames(true);

    session.begin_configuration();
    if !session.can_add_input(&input) {
        session.commit_configuration();
        return Err(anyhow!("cannot add capture input to session").into());
    }
    session.add_input(&input);
    if !session.can_add_output(output) {
        session.commit_configuration();
        return Err(anyhow!("cannot add capture output to session").into());
    }
    session.add_output(output);
    output.set_video_settings(&video_settings(width, height));
    let configured = configure_capture(&device, width, height, refresh_rate_hz);
    session.commit_configuration();
    let configuration = configured?;

    if sink.should_stop(Duration::ZERO) {
        return Ok(None);
    }
    session.start_running();
    drop(configuration);
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
    started(delegate.error().take(), session.is_running())?;
    tracing::info!("Video format set to: YUYV {width}x{height}");
    Ok(Some(running))
}

/// Whether a session that was asked to start runs: the failure recorded
/// meanwhile, else whether it `running`.
fn started(failure: Option<CaptureError>, running: bool) -> Result<()> {
    match (failure, running) {
        (Some(error), _) => Err(error),
        (None, false) => Err(anyhow!("capture session failed to start").into()),
        (None, true) => Ok(()),
    }
}

pub(crate) fn list_devices() -> Vec<crate::DeviceInfo> {
    // SAFETY: a constant NSString AVFoundation defines for the process's life.
    AVCaptureDevice::devices_with_media_type(unsafe { AVMediaTypeVideo })
        .iter()
        .map(|device| crate::DeviceInfo {
            name: device.localized_name().to_string(),
            id: device.unique_id().to_string(),
        })
        .collect()
}

pub(crate) fn list_formats(device_uid: &str) -> Option<Vec<prysm_ports::Mode>> {
    let device = AVCaptureDevice::device_with_unique_id(&NSString::from_str(device_uid))?;
    let mut found = Vec::new();
    for format in device.formats().iter() {
        let Some(description) = format
            .format_description()
            .downcast::<CMVideoFormatDescription>()
        else {
            continue;
        };
        let dimensions = description.get_dimensions();
        let (Ok(width), Ok(height)) = (
            u32::try_from(dimensions.width),
            u32::try_from(dimensions.height),
        ) else {
            continue;
        };
        let mut rates = crate::formats::Rates::default();
        for range in format.video_supported_frame_rate_ranges().iter() {
            rates.within(range.min_frame_rate(), range.max_frame_rate());
        }
        found.extend(
            rates.spans().into_iter().map(|fps| {
                prysm_ports::Mode::new(Span::exactly(width), Span::exactly(height), fps)
            }),
        );
    }
    Some(found)
}

pub(crate) fn run(
    device_uid: Option<&str>,
    width: u32,
    height: u32,
    rate: NonZeroU32,
    sink: &Arc<dyn CaptureSink>,
) -> Result<()> {
    let Some(session) = start_session(device_uid, sink, width, height, rate)? else {
        return Ok(());
    };
    if sink.should_stop(Duration::ZERO) {
        return Ok(());
    }
    sink.ready();
    // The capture thread itself waits, for a stop, a failure (which wakes
    // it) or the next stall check; frames arrive on the session's queue.
    while !session.delegate.failed()
        && !sink.should_stop(session.delegate.until_stall(Instant::now()))
    {
        session.delegate.check_stall(Instant::now());
    }
    let error = session.delegate.error().take();
    drop(session);
    error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_device_lists_nothing_known() {
        assert_eq!(crate::list_formats("prysm-test-missing-device"), None);
    }

    /// The first camera's formats, when there is one: run with
    /// `--ignored --nocapture` on a Mac; it may ask for camera access.
    #[test]
    #[ignore = "needs a camera"]
    fn the_first_camera_lists_its_formats() {
        let device = crate::list_devices().into_iter().next().expect("a camera");
        let formats = crate::list_formats(&device.id).expect("listed");
        println!("{}: {formats:#?}", device.name);
        assert!(!formats.is_empty());
    }

    #[test]
    fn a_failure_while_starting_is_returned_with_its_cause() {
        let disconnected = || CaptureError::Disconnected {
            details: "gone".into(),
            source: None,
        };
        for running in [false, true] {
            assert!(matches!(
                started(Some(disconnected()), running),
                Err(CaptureError::Disconnected { .. })
            ));
        }
        let Err(CaptureError::Backend(error)) = started(None, false) else {
            panic!("a session that does not run");
        };
        assert_eq!(error.to_string(), "capture session failed to start");
        assert!(started(None, true).is_ok());
    }

    #[test]
    fn failed_startup_preserves_capture_error() {
        let result = crate::capture(
            Some("prysm-test-missing-device"),
            640,
            360,
            30.try_into().unwrap(),
            &(Arc::new(crate::device::Discard) as Arc<dyn CaptureSink>),
        );
        assert!(matches!(result, Err(CaptureError::DeviceNotFound { .. })));
    }
}
