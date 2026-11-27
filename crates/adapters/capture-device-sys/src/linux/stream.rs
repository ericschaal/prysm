//! V4L2 buffer allocation and release.
use crate::classify::{Fault, Op, classify, failure};
use crate::{CaptureError, Result};
use std::ffi::c_void;
use std::time::{Duration, Instant};
use std::{io, ptr::NonNull, sync::Arc};
use v4l::{
    Device,
    device::Handle,
    v4l_sys::{
        v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE, v4l2_buffer, v4l2_memory_V4L2_MEMORY_MMAP,
        v4l2_requestbuffers,
    },
    v4l2,
};

const WAITING: &str = "error waiting for a capture frame";
const CAPTURING: &str = "error capturing frame";
const REQUEUING: &str = "failed to requeue capture buffer";

/// One request a stream makes of its device, each with the structure it takes.
enum Request<'a> {
    Reqbufs(&'a mut v4l2_requestbuffers),
    Querybuf(&'a mut v4l2_buffer),
    Qbuf(&'a mut v4l2_buffer),
    Dqbuf(&'a mut v4l2_buffer),
    StreamOn,
    StreamOff,
}

/// Makes a [`Request`] of the device: the kernel's, or a test's.
type Ioctl<'a> = dyn FnMut(Request<'_>) -> io::Result<()> + 'a;

/// Gives a mapping back: consumed when unmapped, returned when not.
type Unmap<'a> = dyn FnMut(Mapping) -> std::result::Result<(), (Mapping, io::Error)> + 'a;

/// One buffer as `mmap` mapped it; made only by [`Stream::allocate`].
struct Mapping {
    pointer: NonNull<u8>,
    length: usize,
}

impl Mapping {
    /// Unmap the buffer, or hand it back with the error.
    fn unmap(self) -> std::result::Result<(), (Self, io::Error)> {
        // SAFETY: `pointer` and `length` are what `mmap` returned (only
        // `allocate` makes a `Mapping`), and the mapping is consumed here, so
        // it is unmapped once; slices of it borrow the stream, which this
        // call's caller holds mutably.
        match unsafe { v4l2::munmap(self.pointer.as_ptr().cast(), self.length) } {
            Ok(()) => Ok(()),
            Err(error) => Err((self, error)),
        }
    }
}

/// A device's capture buffers, mapped for reading.
///
/// - `allocated`: REQBUFS gave the device's buffers to this stream; only
///   `REQBUFS(0)` in [`Stream::release`] gives them back, once every mapping
///   is gone.
/// - `buffers`: one slot per buffer index; `None` once unmapped (only by
///   `release`, which then never maps again).
/// - `active`: STREAMON succeeded, so STREAMOFF stops the stream. A start
///   that fails streams off again to take back the buffers it queued; if
///   that fails too, some stay queued until `REQBUFS(0)` in `release`
///   cancels the queue.
pub(super) struct Stream {
    handle: Arc<Handle>,
    buffers: Vec<Option<Mapping>>,
    allocated: bool,
    active: bool,
    timeout: Duration,
}

/// A failure of `op` on the stream, `doing` what, as [`classify`] reads it.
pub(super) fn failed(op: Op, error: io::Error, doing: &'static str) -> CaptureError {
    failure(classify(op, &error), error, doing)
}

/// A failed DQBUF or QBUF: [`Dequeued::Interrupted`] for a queue error, else
/// the end of capture.
fn queue_failed<T>(op: Op, error: io::Error, doing: &'static str) -> Result<Dequeued<T>> {
    let fault = classify(op, &error);
    let failure = failure(fault, error, doing);
    match fault {
        Fault::QueueError => Ok(Dequeued::Interrupted(failure)),
        Fault::Gone
        | Fault::Busy
        | Fault::Denied
        | Fault::Missing
        | Fault::Rejected
        | Fault::Signalled
        | Fault::Other => Err(failure),
    }
}

/// The outcome of capturing after `release` freed the stream: a failed
/// release is [`CaptureError::Cleanup`] (the device must not be reopened),
/// caused by the capture's own error when there was one.
pub(super) fn released(release: io::Result<()>, result: Result<()>) -> Result<()> {
    match (release, result) {
        (Ok(()), result) => result,
        (Err(release), Ok(())) => Err(CaptureError::Cleanup(release.into())),
        (Err(release), Err(capture)) => Err(CaptureError::Cleanup(
            anyhow::Error::new(capture).context(release),
        )),
    }
}

/// What [`Stream::frame`] got from the device.
pub(super) enum Dequeued<T> {
    /// A buffer, and what was made of it.
    Buffer(T),
    /// No buffer within the timeout.
    Stalled,
    /// DQBUF or QBUF failed with [`Fault::QueueError`]; the stream needs a
    /// restart before the next frame.
    Interrupted(CaptureError),
}

fn buffer(index: u32) -> v4l2_buffer {
    v4l2_buffer {
        index,
        type_: v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE,
        memory: v4l2_memory_V4L2_MEMORY_MMAP,
        // SAFETY: every field is an integer, a raw pointer or a union of them,
        // for which all-zero bytes are valid (and V4L2 wants reserved fields zero).
        ..unsafe { std::mem::zeroed() }
    }
}

/// A request for `count` mapped capture buffers (0 frees them).
fn requested(count: u32) -> v4l2_requestbuffers {
    v4l2_requestbuffers {
        count,
        type_: v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE,
        memory: v4l2_memory_V4L2_MEMORY_MMAP,
        // SAFETY: the remaining fields are integers, for which zero is valid
        // (and V4L2 wants reserved fields zero).
        ..unsafe { std::mem::zeroed() }
    }
}

/// The kernel's ioctl on `handle`'s device, for as long as it is borrowed.
fn kernel(handle: &Handle) -> impl FnMut(Request<'_>) -> io::Result<()> + '_ {
    move |request| {
        let mut kind = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        let (code, value) = raw(request, &mut kind);
        // SAFETY: `raw` pairs each request code with a pointer to the live
        // structure it takes (`kind`, the buffer type, for STREAMON and
        // STREAMOFF), so the kernel reads and writes only within it; the fd is
        // open while `handle` is borrowed.
        unsafe { v4l2::ioctl(handle.fd(), code, value) }
    }
}

/// `request`'s ioctl code and the structure it takes; `kind` (the buffer
/// type) for STREAMON and STREAMOFF.
fn raw<'a>(request: Request<'a>, kind: &'a mut u32) -> (v4l2::vidioc::_IOC_TYPE, *mut c_void) {
    use v4l2::vidioc::{
        VIDIOC_DQBUF, VIDIOC_QBUF, VIDIOC_QUERYBUF, VIDIOC_REQBUFS, VIDIOC_STREAMOFF,
        VIDIOC_STREAMON,
    };
    match request {
        Request::Reqbufs(value) => (VIDIOC_REQBUFS, std::ptr::from_mut(value).cast()),
        Request::Querybuf(value) => (VIDIOC_QUERYBUF, std::ptr::from_mut(value).cast()),
        Request::Qbuf(value) => (VIDIOC_QBUF, std::ptr::from_mut(value).cast()),
        Request::Dqbuf(value) => (VIDIOC_DQBUF, std::ptr::from_mut(value).cast()),
        Request::StreamOn => (VIDIOC_STREAMON, std::ptr::from_mut(kind).cast()),
        Request::StreamOff => (VIDIOC_STREAMOFF, std::ptr::from_mut(kind).cast()),
    }
}

impl Stream {
    pub fn new(device: &Device, timeout: Duration) -> Result<Self> {
        let mut stream = Self {
            handle: device.handle(),
            buffers: Vec::new(),
            allocated: false,
            active: false,
            timeout,
        };
        if let Err(error) = stream.allocate() {
            let allocation = Err(failed(
                Op::Allocate,
                error,
                "failed to allocate capture buffers",
            ));
            return Err(released(stream.release(), allocation)
                .expect_err("a failed allocation stays failed after release"));
        }
        Ok(stream)
    }
    fn allocate(&mut self) -> io::Result<()> {
        let mut ioctl = kernel(&self.handle);
        let mut request = requested(4);
        ioctl(Request::Reqbufs(&mut request))?;
        self.allocated = true;
        if request.count == 0 {
            return Err(io::Error::other("capture device allocated no buffers"));
        }
        for index in 0..request.count {
            let mut info = buffer(index);
            ioctl(Request::Querybuf(&mut info))?;
            if info.length == 0 {
                return Err(io::Error::other("capture device returned an empty buffer"));
            }
            // SAFETY: for MMAP memory QUERYBUF sets `m.offset`, the union member
            // read; mapping `length` bytes at that offset of this device is what
            // V4L2 defines, and the mapping lives until `release` unmaps it.
            // Read-only: a capture queue's buffers are only read here.
            let pointer = unsafe {
                v4l2::mmap(
                    std::ptr::null_mut(),
                    info.length as usize,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    self.handle.fd(),
                    info.m.offset.into(),
                )?
            };
            self.buffers.push(Some(Mapping {
                pointer: NonNull::new(pointer.cast()).expect("mmap returned null"),
                length: info.length as usize,
            }));
        }
        Ok(())
    }
    pub fn start(&mut self) -> io::Result<()> {
        let handle = Arc::clone(&self.handle);
        self.start_with(&mut kernel(&handle))
    }
    /// [`Self::start`] through `ioctl`. A failure streams off again, which
    /// also takes back the buffers queued before it, so a later start can
    /// queue them again.
    fn start_with(&mut self, ioctl: &mut Ioctl<'_>) -> io::Result<()> {
        let mut started = || {
            for index in 0..self.buffers.len() {
                let index = u32::try_from(index).expect("V4L buffer index");
                ioctl(Request::Qbuf(&mut buffer(index)))?;
            }
            ioctl(Request::StreamOn)
        };
        let result = started();
        if result.is_err() {
            // The start's own error is the one reported; a buffer this cannot
            // take back is freed by `release`.
            let _ = ioctl(Request::StreamOff);
        }
        self.active = result.is_ok();
        result
    }
    pub fn stop(&mut self) -> io::Result<()> {
        if self.active {
            kernel(&self.handle)(Request::StreamOff)?;
            self.active = false;
        }
        Ok(())
    }
    /// Dequeue the next buffer, give it to `copy` and queue it again.
    pub fn frame<T>(&mut self, copy: impl FnOnce(&[u8], &v4l2_buffer) -> T) -> Result<Dequeued<T>> {
        let handle = Arc::clone(&self.handle);
        self.frame_with(
            &mut |timeout| handle.poll(libc::POLLIN, timeout),
            &mut kernel(&handle),
            copy,
        )
    }
    /// [`Self::frame`] through `poll` (milliseconds in, ready descriptors
    /// out) and `ioctl`.
    fn frame_with<T>(
        &mut self,
        poll: &mut dyn FnMut(i32) -> io::Result<i32>,
        ioctl: &mut Ioctl<'_>,
        copy: impl FnOnce(&[u8], &v4l2_buffer) -> T,
    ) -> Result<Dequeued<T>> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match poll(i32::try_from(left.as_millis()).unwrap_or(i32::MAX)) {
                Ok(0) => return Ok(Dequeued::Stalled),
                Ok(_) => break,
                Err(error) => match classify(Op::Wait, &error) {
                    Fault::Signalled => {}
                    fault @ (Fault::Gone
                    | Fault::Busy
                    | Fault::Denied
                    | Fault::Missing
                    | Fault::Rejected
                    | Fault::QueueError
                    | Fault::Other) => {
                        return Err(failure(fault, error, WAITING));
                    }
                },
            }
        }
        let mut info = buffer(0);
        // On a queue error `info` names no buffer: one the driver may have
        // taken comes back only with STREAMOFF.
        if let Err(error) = ioctl(Request::Dqbuf(&mut info)) {
            return queue_failed(Op::Dequeue, error, CAPTURING);
        }
        let Some(mapping) = self
            .buffers
            .get(info.index as usize)
            .and_then(Option::as_ref)
        else {
            let error = io::Error::other("capture device returned an invalid buffer index");
            return Err(failed(Op::Dequeue, error, CAPTURING));
        };
        let length = info.bytesused as usize;
        let result = if length > mapping.length {
            Err(io::Error::other("capture frame exceeds its mapped buffer"))
        } else {
            // SAFETY: `pointer` maps `mapping.length >= length` readable bytes
            // (unmapped only by `release`, never called while this runs); the
            // buffer is dequeued, so the driver does not write it until it is
            // queued again below, after `copy` (which cannot keep the slice)
            // returns.
            Ok(copy(
                unsafe { std::slice::from_raw_parts(mapping.pointer.as_ptr(), length) },
                &info,
            ))
        };
        // On a queue error the restart that follows queues every buffer again.
        if let Err(error) = ioctl(Request::Qbuf(&mut buffer(info.index))) {
            return queue_failed(Op::Requeue, error, REQUEUING);
        }
        result
            .map(Dequeued::Buffer)
            .map_err(|error| failed(Op::Dequeue, error, CAPTURING))
    }
    pub fn release(&mut self) -> io::Result<()> {
        let handle = Arc::clone(&self.handle);
        self.release_with(&mut kernel(&handle), &mut Mapping::unmap)
    }
    /// Stop, unmap and free whatever is still held, through `ioctl` and
    /// `unmap`. A device that is gone ([`Fault::Gone`]) has nothing left to
    /// stop or free, so that counts as released; its mappings are still
    /// unmapped, which needs no device.
    fn release_with(&mut self, ioctl: &mut Ioctl<'_>, unmap: &mut Unmap<'_>) -> io::Result<()> {
        let mut ioctl = |request| match ioctl(request) {
            Err(error) if classify(Op::Release, &error) == Fault::Gone => Ok(()),
            result => result,
        };
        let mut failure = None;
        if self.active {
            match ioctl(Request::StreamOff) {
                Ok(()) => self.active = false,
                Err(error) => {
                    failure = Some(error);
                }
            }
        }
        for slot in &mut self.buffers {
            if let Some(mapping) = slot.take()
                && let Err((mapping, error)) = unmap(mapping)
            {
                *slot = Some(mapping);
                failure.get_or_insert(error);
            }
        }
        if self.allocated && self.buffers.iter().all(Option::is_none) {
            match ioctl(Request::Reqbufs(&mut requested(0))) {
                Ok(()) => self.allocated = false,
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        // Explicit release carries errors; Drop is only the non-panicking fallback.
        if let Err(error) = self.release() {
            tracing::error!(%error, "capture buffer cleanup failed");
        }
    }
}

#[cfg(test)]
mod tests;
