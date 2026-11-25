//! The Objective-C object AVFoundation and Foundation call back: sample
//! buffers on the session's queue, failure notifications on whichever thread
//! posts them.
use std::num::NonZeroU32;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::anyhow;
use av_foundation::capture_device::AVCaptureDeviceWasDisconnectedNotification;
use av_foundation::capture_output_base::AVCaptureOutput;
use av_foundation::capture_session::{AVCaptureConnection, AVCaptureSessionErrorKey};
use av_foundation::capture_video_data_output::AVCaptureVideoDataOutputSampleBufferDelegate;
use core_foundation::base::TCFType;
use core_media::sample_buffer::{CMSampleBuffer, CMSampleBufferRef};
use core_video::pixel_buffer::CVPixelBuffer;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_foundation::{
    NSError, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol, NSString,
};

use super::pixel_buffer::{captured_at, frame_from_pixel_buffer};
use crate::health::{Event, Health, Verdict, stall_timeout};
use crate::{BufferRejection, CaptureError, CaptureSink, FramePool};

/// What delivering frames needs, until capture ends.
pub(super) struct CaptureState {
    sink: Arc<dyn CaptureSink>,
    pool: FramePool,
    stats: crate::trace_stats::TraceStats,
    health: Health,
    /// When the last sample arrived, or capture started, or the last stall
    /// was counted.
    last_sample: Instant,
    /// How long without a sample counts as a stall.
    stall: Duration,
}

pub(super) struct DelegateIvars {
    pub(super) capture: Mutex<Option<CaptureState>>,
    /// A runtime error, disconnection or run of unusable buffers; capture
    /// stops once it is set.
    error: Mutex<Option<CaptureError>>,
}

// The delegate's methods run with `&self` on AVFoundation's serial queue and
// on any thread that posts a notification, while the capture thread reads the
// same ivars: only `Sync` state may live in them.
const _: () = {
    const fn shared<T: Sync>() {}
    shared::<DelegateIvars>();
};

define_class!(
    // SAFETY: NSObject is made to be subclassed and asks nothing of a
    // subclass; the ivars' `Drop` (below) runs from `dealloc` and cannot panic.
    #[unsafe(super(NSObject))]
    #[name = "PrysmCaptureSampleBufferDelegate"]
    #[ivars = DelegateIvars]
    pub(super) struct Delegate;

    // SAFETY: NSObject, the superclass, already conforms to NSObjectProtocol.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: the protocol's one method implemented here has the signature
    // av-foundation 0.8.1 declares for it (`capture_video_data_output.rs`);
    // AVFoundation calls it on the queue given to `setSampleBufferDelegate:`.
    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Delegate {
        // SAFETY: `captureOutput:didOutputSampleBuffer:fromConnection:` takes
        // an output, a `CMSampleBufferRef` and a connection, returns nothing,
        // and runs on the session's serial queue.
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn capture_output_did_output_sample_buffer(
            &self,
            _capture_output: &AVCaptureOutput,
            sample_buffer: CMSampleBufferRef,
            _connection: &AVCaptureConnection,
        ) {
            self.guarded("captureOutput:didOutputSampleBuffer:fromConnection:", || {
                // SAFETY: AVFoundation passes a valid sample buffer that it
                // keeps alive for this call; the Get-rule wrapper retains it
                // for as long as the wrapper lives and releases only that retain.
                let sample = unsafe { CMSampleBuffer::wrap_under_get_rule(sample_buffer) };
                self.deliver(&sample, Instant::now());
            });
        }
    }

    impl Delegate {
        // SAFETY: `captureFailed:` takes the `NSNotification *` that
        // `addObserver:selector:name:object:` passes (`observe_failure`) and
        // returns nothing; it runs on whichever thread posts the notification.
        #[unsafe(method(captureFailed:))]
        fn capture_failed(&self, notification: &NSNotification) {
            self.guarded("captureFailed:", || self.fail_and_wake(failure(notification)));
        }

        // Not guarded: setting the ivars and NSObject's `init` do not panic.
        // SAFETY: `init` takes the allocation and returns it initialised and
        // retained, or nil: `Allocated<Self>` in, `Option<Retained<Self>>` out.
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Option<Retained<Self>> {
            let this = this.set_ivars(DelegateIvars {
                capture: Mutex::new(None),
                error: Mutex::new(None),
            });
            // SAFETY: `this` is a fresh allocation with its ivars set, and
            // NSObject's `init` returns the receiver (retained) or nil, as the
            // `Option<Retained<Self>>` return type says.
            unsafe { msg_send![super(this), init] }
        }
    }
);

impl Delegate {
    /// A delegate delivering to `sink`, judging capture at `rate` from `now`.
    pub(super) fn new(
        sink: Arc<dyn CaptureSink>,
        rate: NonZeroU32,
        now: Instant,
    ) -> Retained<Self> {
        // SAFETY: `init` is the method defined above, which takes the fresh
        // allocation and returns it initialised; NSObject's `init` never
        // returns nil, so the non-optional `Retained` holds.
        let this: Retained<Self> = unsafe { msg_send![Self::alloc(), init] };
        *this.ivars().capture.lock().expect("capture mutex poisoned") = Some(CaptureState {
            sink,
            pool: FramePool::default(),
            stats: crate::trace_stats::TraceStats::new("avfoundation_copy"),
            health: Health::new(rate),
            last_sample: now,
            stall: stall_timeout(rate),
        });
        this
    }

    /// Runs `body`, the Objective-C method `method`. objc2 declares the method
    /// `extern "C-unwind"`, so a panic would unwind into the AVFoundation or
    /// Foundation code that called it; instead it is caught here and becomes
    /// the capture's failure, with the panic's message as its cause.
    fn guarded(&self, method: &'static str, body: impl FnOnce()) {
        // Unwind safe: what a panic leaves half-changed is behind the frame
        // lock, which it poisons and nothing reads again.
        let Err(payload) = panic::catch_unwind(AssertUnwindSafe(body)) else {
            return;
        };
        let message = payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic without a message".to_owned());
        // A payload's destructor may panic in turn; that second payload is leaked.
        if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| drop(payload))) {
            std::mem::forget(payload);
        }
        self.fail_and_wake(CaptureError::Backend(
            anyhow!(message).context(format!("{method} panicked")),
        ));
    }

    /// Copies `sample_buffer`, which arrived at `now`, into a frame for the
    /// sink, unless capture is ending; an unusable one goes to the health
    /// rule, which may fail capture.
    fn deliver(&self, sample_buffer: &CMSampleBuffer, now: Instant) {
        let captured = captured_at(sample_buffer.get_presentation_time_stamp());
        let pixel_buffer = sample_buffer
            .get_image_buffer()
            .and_then(|image_buffer| image_buffer.downcast::<CVPixelBuffer>())
            .ok_or(BufferRejection::NoPixelBuffer);
        // Poisoned only by a panic in this method, which `guarded` recorded as
        // the failure that stops capture: no frame is delivered after it.
        let Ok(mut capture) = self.ivars().capture.lock() else {
            return;
        };
        let Some(capture) = capture.as_mut() else {
            return;
        };
        capture.last_sample = now;
        let started = capture.stats.start();
        let event = match pixel_buffer
            .and_then(|pixel_buffer| frame_from_pixel_buffer(&pixel_buffer, &mut capture.pool))
        {
            Ok(frame) => {
                capture
                    .stats
                    .record(started, frame.len(), Some(capture.pool.stats()));
                capture.sink.frame(frame.with_captured(captured));
                Event::Frame
            }
            Err(reason) => {
                capture.stats.failure();
                Event::Rejected(reason)
            }
        };
        self.judge(capture, event, now);
    }

    /// Counts a stall when no sample arrived for the stall timeout before
    /// `now`, so a source that went quiet after a few unusable buffers does
    /// not keep their run open; then waits a whole timeout for the next.
    /// Called by the capture thread.
    pub(super) fn check_stall(&self, now: Instant) {
        // Poisoned only by a callback's panic, recorded as the failure that
        // stops capture.
        let Ok(mut capture) = self.ivars().capture.lock() else {
            return;
        };
        let Some(capture) = capture.as_mut() else {
            return;
        };
        if now.saturating_duration_since(capture.last_sample) >= capture.stall {
            capture.last_sample = now;
            self.judge(capture, Event::Stalled, now);
        }
    }

    /// How long from `now` until [`Self::check_stall`] can count a stall:
    /// what the capture thread waits for, besides a stop or a failure.
    pub(super) fn until_stall(&self, now: Instant) -> Duration {
        let Ok(capture) = self.ivars().capture.lock() else {
            return Duration::ZERO;
        };
        capture.as_ref().map_or(Duration::ZERO, |capture| {
            (capture.last_sample + capture.stall).saturating_duration_since(now)
        })
    }

    /// Acts on what `capture`'s health makes of `event` at `now`; a failure
    /// wakes the capture thread.
    fn judge(&self, capture: &mut CaptureState, event: Event, now: Instant) {
        match capture.health.on(event, now) {
            Verdict::Fail(error) => {
                self.fail(error);
                capture.sink.wake();
            }
            // A quiet session has nothing to restart: it delivers again when
            // its source does.
            Verdict::Continue | Verdict::Restart => {}
        }
    }

    /// The failure that stops capture. Only whole values are stored and taken
    /// under this lock, so a poisoned one still holds a whole value.
    pub(super) fn error(&self) -> MutexGuard<'_, Option<CaptureError>> {
        self.ivars()
            .error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Records `error`, unless a failure is recorded already.
    fn fail(&self, error: CaptureError) {
        self.error().get_or_insert(error);
    }

    /// [`Self::fail`], then wake the capture thread, which waits for it. Not
    /// called with the capture lock held.
    fn fail_and_wake(&self, error: CaptureError) {
        self.fail(error);
        let capture = self
            .ivars()
            .capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(capture) = capture.as_ref() {
            capture.sink.wake();
        }
    }

    pub(super) fn failed(&self) -> bool {
        self.error().is_some()
    }

    pub(super) fn observe_failure(&self, name: &NSString, object: &AnyObject) {
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

/// Runs from the class's `dealloc`, called by Objective-C, so nothing here or
/// in the ivars' drops may panic: removing an observer does not, and the
/// ivars hold only buffers, the recorded error, counters (whose drop logs
/// them) and a clone of the sink's `Arc` that `run`'s caller outlives.
impl Drop for Delegate {
    fn drop(&mut self) {
        // SAFETY: self is the live observer registered by observe_failure.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self) };
    }
}

/// The failure a session runtime error or device disconnection `notification`
/// reports; the session's `NSError`, when there is one, is its source.
fn failure(notification: &NSNotification) -> CaptureError {
    let error = format!("capture failed: {}", notification.name());
    // SAFETY: This notification name is a process-lifetime Foundation string.
    if &*notification.name() == unsafe { AVCaptureDeviceWasDisconnectedNotification } {
        return CaptureError::Disconnected {
            details: error,
            source: None,
        };
    }
    // SAFETY: This dictionary key is a process-lifetime Foundation string.
    let source = notification
        .userInfo()
        .and_then(|info| info.objectForKey(unsafe { AVCaptureSessionErrorKey }))
        .and_then(|value| value.downcast::<NSError>().ok());
    CaptureError::Backend(match source {
        Some(source) => anyhow::Error::new(source).context(error),
        None => anyhow!(error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use av_foundation::capture_session::AVCaptureSessionRuntimeErrorNotification;
    use objc2_foundation::NSDictionary;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn runtime_notification_preserves_native_error() {
        let object = NSObject::new();
        let delegate = delegate_for(Arc::new(crate::device::Discard));
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

    #[test]
    fn capture_failure_notifications_stop_capture_and_release_the_sink() {
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
            let woken = Arc::new(Count::default());
            let delegate = delegate_for(woken.clone());
            delegate.observe_failure(name, &object);

            // SAFETY: These live NSObject instances identify the notification source;
            // the observer only reads the notification name, not its object.
            unsafe {
                NSNotificationCenter::defaultCenter()
                    .postNotificationName_object(name, Some(&unrelated));
                assert!(!delegate.failed(), "another capture must not stop this one");
                NSNotificationCenter::defaultCenter()
                    .postNotificationName_object(name, Some(&object));
            }
            assert!(delegate.failed());
            assert_eq!(
                woken.1.load(Ordering::Relaxed),
                1,
                "the capture thread is woken"
            );
            let error = delegate.ivars().error.lock().unwrap().take().unwrap();
            assert!(error.to_string().contains(&name.to_string()));
            // SAFETY: a constant NSString AVFoundation defines for the process's life.
            let disconnected = name == unsafe { AVCaptureDeviceWasDisconnectedNotification };
            assert_eq!(
                matches!(error, CaptureError::Disconnected { .. }),
                disconnected
            );
            drop(delegate);
            assert_eq!(
                Arc::strong_count(&woken),
                1,
                "the delegate released the sink"
            );
        }
    }

    /// A sample holding a pixel buffer of `format`, as AVFoundation delivers one.
    fn sample(format: u32) -> CMSampleBuffer {
        use core_media::format_description::CMVideoFormatDescription;
        use core_media::sample_buffer::CMSampleTimingInfo;
        use core_media::time::CMTime;
        use core_video::image_buffer::TCVImageBuffer;
        let pixels = CVPixelBuffer::new(format, 4, 2, None)
            .unwrap()
            .as_image_buffer();
        let description = CMVideoFormatDescription::from_image_buffer(&pixels).unwrap();
        let timing = CMSampleTimingInfo {
            duration: CMTime::default(),
            presentationTimeStamp: CMTime::default(),
            decodeTimeStamp: CMTime::default(),
        };
        CMSampleBuffer::from_image_buffer_ready(&pixels, &description, &timing).unwrap()
    }

    /// A sink that counts the frames it gets and its wake-ups.
    #[derive(Default)]
    struct Count(AtomicUsize, AtomicUsize);

    impl CaptureSink for Count {
        fn ready(&self) {}
        fn frame(&self, _: crate::Frame) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn should_stop(&self, _: Duration) -> bool {
            false
        }
        fn wake(&self) {
            self.1.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_run_of_unusable_pixel_buffers_fails_capture() {
        use core_video::pixel_buffer::kCVPixelFormatType_32BGRA;
        let start = Instant::now();
        let sink = Arc::new(Count::default());
        let delegate = Delegate::new(sink.clone(), RATE, start);
        let unusable = sample(kCVPixelFormatType_32BGRA);
        // A second's worth, then the rest of three seconds at the rate.
        let period = Duration::from_secs(1) / RATE.get();
        for frame in 0..RATE.get() * 3 {
            delegate.deliver(&unusable, start + period * frame);
            assert!(!delegate.failed(), "frame {frame}: within the grace");
        }
        delegate.deliver(&unusable, start + Duration::from_secs(3));
        assert_eq!(
            sink.1.load(Ordering::Relaxed),
            1,
            "the capture thread is woken"
        );
        let error = delegate.error().take().expect("capture failed");
        let CaptureError::Unusable { rejected, source } = &error else {
            panic!("expected unusable buffers, got {error:?}");
        };
        assert_eq!(*rejected, RATE.get() * 3 + 1);
        assert_eq!(*source, BufferRejection::PixelFormat(*b"BGRA"));
    }

    #[test]
    fn a_frame_or_a_quiet_spell_ends_a_run_of_unusable_buffers() {
        use core_video::pixel_buffer::{
            kCVPixelFormatType_32BGRA, kCVPixelFormatType_422YpCbCr8_yuvs,
        };
        let start = Instant::now();
        let sink = Arc::new(Count::default());
        let delegate = Delegate::new(sink.clone(), RATE, start);
        let (unusable, usable) = (
            sample(kCVPixelFormatType_32BGRA),
            sample(kCVPixelFormatType_422YpCbCr8_yuvs),
        );
        let stall = stall_timeout(RATE);
        // Two seconds of bad buffers, ten times, each run broken by a frame
        // or a stall before it lasts the three that fail capture.
        let mut now = start;
        for run in 0..10 {
            for _ in 0..RATE.get() * 2 {
                delegate.deliver(&unusable, now);
                now += Duration::from_secs(1) / RATE.get();
            }
            if run % 2 == 0 {
                delegate.deliver(&usable, now);
            } else {
                delegate.check_stall(now + stall / 2);
                delegate.check_stall(now + stall);
                now += stall;
            }
        }
        assert!(!delegate.failed(), "{:?}", delegate.error().take());
        assert_eq!(sink.0.load(Ordering::Relaxed), 5);
        assert_eq!(sink.1.load(Ordering::Relaxed), 0, "nothing failed");
    }

    const RATE: NonZeroU32 = NonZeroU32::new(30).unwrap();

    fn delegate_for(sink: Arc<dyn CaptureSink>) -> Retained<Delegate> {
        Delegate::new(sink, RATE, Instant::now())
    }

    #[test]
    fn a_panicking_callback_fails_capture_instead_of_unwinding() {
        let sink: Arc<dyn CaptureSink> = Arc::new(crate::device::Discard);
        let delegate = delegate_for(sink.clone());
        delegate.guarded("first:", || {
            let _capture = delegate.ivars().capture.lock();
            panic!("broken {}", "invariant");
        });
        delegate.guarded("second:", || panic!("later"));
        assert!(
            delegate.ivars().capture.lock().is_err(),
            "frames stop: the panic poisoned their state"
        );
        let error = delegate.error().take().unwrap();
        assert_eq!(
            prysm_ports::chain(&error),
            "first: panicked: broken invariant",
            "the first failure is kept, its message the cause"
        );
        drop(delegate);
        assert_eq!(
            Arc::strong_count(&sink),
            1,
            "the delegate released the sink"
        );

        let delegate = delegate_for(Arc::new(crate::device::Discard));
        delegate.guarded("static:", || panic!("a static message"));
        let error = delegate.error().take().unwrap();
        assert_eq!(
            prysm_ports::chain(&error),
            "static: panicked: a static message"
        );
    }
}
