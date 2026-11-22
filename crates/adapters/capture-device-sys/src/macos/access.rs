//! macOS's camera access: asked once, waited for, then decided.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use av_foundation::capture_device::{
    AVAuthorizationStatus, AVAuthorizationStatusDenied, AVAuthorizationStatusNotDetermined,
    AVAuthorizationStatusRestricted, AVCaptureDevice,
};
use av_foundation::media_format::AVMediaTypeVideo;

use super::FAILURE_POLL;
use crate::{CameraAccess, CaptureError, CaptureSink, Result};

/// Where macOS's camera access request stands.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Request {
    NotMade,
    Answered,
    TimedOut,
}

/// What an authorization status means for opening: `Ok(true)` to open,
/// `Ok(false)` to ask first. A status still undecided after the request means
/// macOS answered without asking, or nobody answered. A status this crate
/// does not know is left to AVFoundation.
#[allow(non_upper_case_globals)] // Names come from AVFoundation.
fn access(
    status: AVAuthorizationStatus,
    request: Request,
) -> std::result::Result<bool, CameraAccess> {
    match (status, request) {
        (AVAuthorizationStatusNotDetermined, Request::NotMade) => Ok(false),
        (AVAuthorizationStatusNotDetermined, Request::Answered) => Err(CameraAccess::NotAsked),
        (AVAuthorizationStatusNotDetermined, Request::TimedOut) => Err(CameraAccess::Unanswered),
        (AVAuthorizationStatusDenied, _) => Err(CameraAccess::Denied),
        (AVAuthorizationStatusRestricted, _) => Err(CameraAccess::Restricted),
        _ => Ok(true),
    }
}

/// Whether the process may capture video, asking macOS when it has not
/// decided; `Ok(false)` when `sink` stops while waiting for the answer.
pub(super) fn camera_access(sink: &Arc<dyn CaptureSink>) -> Result<bool> {
    // SAFETY: AVMediaTypeVideo is a constant NSString that AVFoundation
    // defines for the process's life; reading the extern static is all.
    let video = unsafe { AVMediaTypeVideo };
    let status = || AVCaptureDevice::authorization_status_for_media_type(video);
    if access(status(), Request::NotMade).map_err(CaptureError::NoCameraAccess)? {
        return Ok(true);
    }
    tracing::info!("Asking macOS for camera access");
    let answered = Arc::new(AtomicBool::new(false));
    AVCaptureDevice::request_access_for_media_type(video, {
        let answered = answered.clone();
        move |_| answered.store(true, Ordering::Release)
    });
    let Some(request) = wait_for_answer(&answered, sink.as_ref(), crate::CAMERA_ACCESS_WAIT) else {
        return Ok(false);
    };
    access(status(), request).map_err(CaptureError::NoCameraAccess)
}

/// Waits up to `limit` for `answered`: `Answered` or `TimedOut`, or `None`
/// when `sink` stops first.
fn wait_for_answer(
    answered: &AtomicBool,
    sink: &dyn CaptureSink,
    limit: Duration,
) -> Option<Request> {
    let asked = Instant::now();
    loop {
        if answered.load(Ordering::Acquire) {
            return Some(Request::Answered);
        }
        if asked.elapsed() >= limit {
            return Some(Request::TimedOut);
        }
        if sink.should_stop(FAILURE_POLL) {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Frame;
    use av_foundation::capture_device::AVAuthorizationStatusAuthorized;

    #[test]
    fn camera_access_opens_asks_once_or_refuses() {
        use AVAuthorizationStatusNotDetermined as NotDetermined;
        assert_eq!(
            access(NotDetermined, Request::NotMade),
            Ok(false),
            "ask first"
        );
        assert_eq!(
            access(NotDetermined, Request::Answered),
            Err(CameraAccess::NotAsked)
        );
        assert_eq!(
            access(NotDetermined, Request::TimedOut),
            Err(CameraAccess::Unanswered)
        );
        for request in [Request::NotMade, Request::Answered, Request::TimedOut] {
            assert_eq!(
                access(AVAuthorizationStatusDenied, request),
                Err(CameraAccess::Denied)
            );
            assert_eq!(
                access(AVAuthorizationStatusRestricted, request),
                Err(CameraAccess::Restricted)
            );
            assert_eq!(access(AVAuthorizationStatusAuthorized, request), Ok(true));
            assert_eq!(
                access(42, request),
                Ok(true),
                "unknown: AVFoundation decides"
            );
        }
    }

    /// A sink asked to stop.
    struct Stopped;

    impl CaptureSink for Stopped {
        fn ready(&self) {}
        fn frame(&self, _: Frame) {}
        fn should_stop(&self, _: Duration) -> bool {
            true
        }
    }

    #[test]
    fn waiting_for_access_ends_answered_timed_out_or_stopped() {
        let limit = FAILURE_POLL * 2;
        let answered = AtomicBool::new(true);
        assert_eq!(
            wait_for_answer(&answered, &crate::device::Discard, limit),
            Some(Request::Answered)
        );

        let unanswered = AtomicBool::new(false);
        let waiting = Instant::now();
        assert_eq!(
            wait_for_answer(&unanswered, &crate::device::Discard, limit),
            Some(Request::TimedOut)
        );
        assert!(waiting.elapsed() >= limit, "waits for the whole limit");

        assert_eq!(
            wait_for_answer(&unanswered, &Stopped, crate::CAMERA_ACCESS_WAIT),
            None
        );
    }
}
