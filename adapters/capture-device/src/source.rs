//! One opened device. Its [`OwnerThread`] runs the blocking native capture
//! (`prysm_capture_device_sys::capture`), which publishes readiness and the newest frame
//! through a [`CaptureSink`]; the capture's result becomes the terminal
//! failure and the release result. The session thread only waits with a
//! deadline.
use prysm_capture_device_sys::{CaptureError, CaptureSink};
use prysm_core::Frame;
use prysm_ports::{Error, FrameSource, OwnerThread, Pull, Result, Worker};
use std::sync::Arc;
use std::time::Duration;

/// The native capture as the adapter runs it: blocks until the sink asks to
/// stop or capture fails, then releases. Private test seam.
pub(crate) type Capture = Box<dyn FnOnce(&Arc<dyn CaptureSink>) -> CaptureResult + Send>;
pub(crate) type CaptureResult = Result<(), CaptureError>;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadlines {
    /// How long `open` waits for native activation.
    pub ready: Duration,
    /// How long `close` waits for the capture to release the device. Shorter
    /// than the runtime's session stop deadline so the adapter reports first.
    /// V4L2 notices a stop within its poll timeout (1 s plus one frame
    /// period); `AVFoundation` within 50 ms.
    pub release: Duration,
}

impl Deadlines {
    pub(crate) const DEVICE: Self = Self {
        ready: Duration::from_secs(10),
        release: Duration::from_secs(3),
    };
}

/// The owner thread as the native capture sees it.
struct Sink(Worker);

impl CaptureSink for Sink {
    fn ready(&self) {
        self.0.ready();
    }

    fn frame(&self, frame: Frame) {
        self.0.offer(frame);
    }

    fn should_stop(&self, wait: Duration) -> bool {
        self.0.sleep(wait);
        self.0.stopped()
    }
}

/// Run `capture` and turn its result into the source's failure and release.
fn run(capture: Capture, worker: &Worker) -> Result<()> {
    let sink: Arc<dyn CaptureSink> = Arc::new(Sink(worker.clone()));
    match capture(&sink) {
        Ok(()) => Ok(()),
        Err(error @ CaptureError::Cleanup(_)) => {
            let error = Error::adapter("video device release is unconfirmed", error);
            worker.fail(error.clone());
            Err(error)
        }
        Err(error) => {
            worker.fail(Error::adapter("video device capture failed", error));
            Ok(())
        }
    }
}

pub(crate) struct DeviceFrames {
    thread: OwnerThread,
}

impl DeviceFrames {
    /// Start `capture` and wait for native readiness. On any failure the
    /// device is released first.
    pub(crate) fn start(capture: Capture, deadlines: Deadlines) -> Result<Self> {
        let thread = OwnerThread::spawn(
            "prysm-capture-device",
            "video device capture thread",
            deadlines.release,
            move |worker| run(capture, worker),
        )?;
        match thread.wait_ready(deadlines.ready) {
            Ok(true) => Ok(Self { thread }),
            Ok(false) => {
                let late = format!("video device was not ready within {:?}", deadlines.ready);
                Err(match thread.close() {
                    Ok(()) => Error::other(late),
                    Err(unreleased) => Error::adapter(late, unreleased),
                })
            }
            // The capture has returned, so this release is immediate; an
            // unconfirmed one is already the failure.
            Err(failure) => {
                let _ = thread.close();
                Err(failure)
            }
        }
    }

    /// The frame published and not yet taken.
    #[cfg(test)]
    pub(crate) fn pending_frame(&self) -> Option<Frame> {
        self.thread.peek()
    }
}

impl FrameSource for DeviceFrames {
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        self.thread.next_frame(timeout)
    }

    fn close(self: Box<Self>) -> Result<()> {
        self.thread.close()
    }
}
