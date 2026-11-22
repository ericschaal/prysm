//! Explicit V4L buffer lifetime. Unlike the upstream stream, cleanup never panics.
use crate::{CaptureError, Result};
use std::ffi::c_void;
use std::os::fd::RawFd;
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

/// What capture was doing when the stream failed.
const WAITING: &str = "error waiting for a capture frame";
const CAPTURING: &str = "error capturing frame";
const REQUEUING: &str = "failed to requeue capture buffer";

/// A raw ioctl on the stream's device, as `v4l2::ioctl` makes it: the request
/// and a pointer to the structure that request takes.
type Ioctl<'a> = dyn FnMut(v4l2::vidioc::_IOC_TYPE, *mut c_void) -> io::Result<()> + 'a;

struct Mapping {
    pointer: NonNull<u8>,
    length: usize,
}

/// A device's capture buffers, mapped for reading.
///
/// - `allocated`: REQBUFS gave the device's buffers to this stream; only
///   `REQBUFS(0)` in [`Stream::release`] gives them back, once every mapping
///   is gone.
/// - `buffers`: one slot per buffer index; `None` once unmapped (only by
///   `release`, which then never maps again).
/// - `active`: STREAMON succeeded. When it is false no buffer is queued:
///   a start that fails undoes itself with STREAMOFF.
pub(super) struct Stream {
    handle: Arc<Handle>,
    buffers: Vec<Option<Mapping>>,
    allocated: bool,
    active: bool,
    timeout: Duration,
}

/// Whether `error` from a V4L2 ioctl means the device is gone (unplugged or
/// reset). Once a node is unregistered the V4L2 core answers every ioctl with
/// ENODEV without calling the driver, and unregistering stops streaming and
/// frees the queue's buffers (at the latest when the fd closes). Nothing else
/// counts: EIO, EPROTO or ESHUTDOWN can come from a device still registered,
/// whose buffers may still be held.
fn device_gone(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENODEV)
}

/// A failure of the running stream, `doing` what: [`CaptureError::Disconnected`]
/// when the device is gone, else a backend error.
pub(super) fn failed(error: io::Error, doing: &'static str) -> CaptureError {
    if device_gone(&error) {
        CaptureError::Disconnected {
            details: doing.into(),
            source: Some(error),
        }
    } else {
        anyhow::Error::new(error).context(doing).into()
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

/// Whether `error` from `VIDIOC_DQBUF` or `VIDIOC_QBUF` may be temporary:
/// EIO, which V4L2 also uses for problems like signal loss (see
/// [`crate::health`]).
fn interrupted(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::EIO)
}

/// What [`Stream::frame`] got from the device.
pub(super) enum Dequeued<T> {
    /// A buffer, and what was made of it.
    Buffer(T),
    /// No buffer within the timeout.
    Stalled,
    /// DQBUF or QBUF failed with a possibly temporary error; the stream needs
    /// a restart before the next frame.
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

/// The kernel's ioctl on `fd`, for the `_with` methods.
fn kernel(fd: RawFd) -> impl FnMut(v4l2::vidioc::_IOC_TYPE, *mut c_void) -> io::Result<()> {
    // SAFETY: each `_with` method passes only the requests its documentation
    // names, each with a pointer to the live structure that request takes;
    // `fd` is the stream's, open for as long as the stream (which outlives
    // the call) holds its handle.
    move |request, value| unsafe { v4l2::ioctl(fd, request, value) }
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
            let allocation = Err(CaptureError::Backend(error.into()));
            return Err(released(stream.release(), allocation)
                .expect_err("a failed allocation stays failed after release"));
        }
        Ok(stream)
    }
    /// # Safety
    /// `value` is the structure `request` takes, so the kernel reads and
    /// writes only within it.
    unsafe fn ioctl<T>(&self, request: v4l2::vidioc::_IOC_TYPE, value: &mut T) -> io::Result<()> {
        // SAFETY: the caller pairs `request` with its structure; the fd is open
        // for as long as `self.handle` lives.
        unsafe { v4l2::ioctl(self.handle.fd(), request, std::ptr::from_mut(value).cast()) }
    }
    fn request_buffers(&self, count: u32) -> io::Result<u32> {
        let mut request = v4l2_requestbuffers {
            count,
            type_: v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE,
            memory: v4l2_memory_V4L2_MEMORY_MMAP,
            // SAFETY: the remaining fields are integers, for which zero is valid
            // (and V4L2 wants reserved fields zero).
            ..unsafe { std::mem::zeroed() }
        };
        // SAFETY: REQBUFS takes a `v4l2_requestbuffers`.
        unsafe { self.ioctl(v4l2::vidioc::VIDIOC_REQBUFS, &mut request) }?;
        Ok(request.count)
    }
    fn allocate(&mut self) -> io::Result<()> {
        let count = self.request_buffers(4)?;
        self.allocated = true;
        if count == 0 {
            return Err(io::Error::other("capture device allocated no buffers"));
        }
        for index in 0..count {
            let mut info = buffer(index);
            // SAFETY: QUERYBUF takes a `v4l2_buffer`.
            unsafe { self.ioctl(v4l2::vidioc::VIDIOC_QUERYBUF, &mut info) }?;
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
    /// Queue every buffer and stream on.
    pub fn start(&mut self) -> io::Result<()> {
        self.start_with(&mut kernel(self.handle.fd()))
    }
    /// [`Self::start`] through `ioctl`: QBUF with a `v4l2_buffer`, STREAMON
    /// and STREAMOFF with the buffer type. A failure streams off again, which
    /// also takes back the buffers queued before it, so a later start can
    /// queue them again.
    fn start_with(&mut self, ioctl: &mut Ioctl<'_>) -> io::Result<()> {
        let mut kind = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        let mut started = || {
            for index in 0..self.buffers.len() {
                let index = u32::try_from(index).expect("V4L buffer index");
                ioctl(
                    v4l2::vidioc::VIDIOC_QBUF,
                    std::ptr::from_mut(&mut buffer(index)).cast(),
                )?;
            }
            ioctl(
                v4l2::vidioc::VIDIOC_STREAMON,
                std::ptr::from_mut(&mut kind).cast(),
            )
        };
        let result = started();
        if result.is_err() {
            // The start's own error is the one reported; a buffer this cannot
            // take back is freed by `release`.
            let _ = ioctl(
                v4l2::vidioc::VIDIOC_STREAMOFF,
                std::ptr::from_mut(&mut kind).cast(),
            );
        }
        self.active = result.is_ok();
        result
    }
    pub fn stop(&mut self) -> io::Result<()> {
        if self.active {
            let mut kind = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
            // SAFETY: STREAMOFF takes the buffer type, a `u32`.
            unsafe { self.ioctl(v4l2::vidioc::VIDIOC_STREAMOFF, &mut kind) }?;
            self.active = false;
        }
        Ok(())
    }
    /// Dequeue the next buffer, give it to `copy` and queue it again.
    pub fn frame<T>(&mut self, copy: impl FnOnce(&[u8], &v4l2_buffer) -> T) -> Result<Dequeued<T>> {
        let handle = Arc::clone(&self.handle);
        self.frame_with(
            &mut |timeout| handle.poll(libc::POLLIN, timeout),
            &mut kernel(handle.fd()),
            copy,
        )
    }
    /// [`Self::frame`] through `poll` (milliseconds in, ready descriptors
    /// out) and `ioctl`: DQBUF and QBUF, each with a `v4l2_buffer`.
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
                // A signal handler ran on this thread: poll(2) is never
                // restarted, so wait again for what is left.
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(failed(error, WAITING)),
            }
        }
        let mut info = buffer(0);
        match ioctl(
            v4l2::vidioc::VIDIOC_DQBUF,
            std::ptr::from_mut(&mut info).cast(),
        ) {
            Ok(()) => {}
            // `info` names no buffer: one the driver may have taken comes
            // back only with STREAMOFF.
            Err(error) if interrupted(&error) => {
                return Ok(Dequeued::Interrupted(failed(error, CAPTURING)));
            }
            Err(error) => return Err(failed(error, CAPTURING)),
        }
        let Some(mapping) = self
            .buffers
            .get(info.index as usize)
            .and_then(Option::as_ref)
        else {
            let error = io::Error::other("capture device returned an invalid buffer index");
            return Err(failed(error, CAPTURING));
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
        match ioctl(
            v4l2::vidioc::VIDIOC_QBUF,
            std::ptr::from_mut(&mut buffer(info.index)).cast(),
        ) {
            Ok(()) => {}
            // The restart that follows queues every buffer again.
            Err(error) if interrupted(&error) => {
                return Ok(Dequeued::Interrupted(failed(error, REQUEUING)));
            }
            Err(error) => return Err(failed(error, REQUEUING)),
        }
        result
            .map(Dequeued::Buffer)
            .map_err(|error| failed(error, CAPTURING))
    }
    pub fn release(&mut self) -> io::Result<()> {
        let fd = self.handle.fd();
        self.release_with(
            &mut kernel(fd),
            // SAFETY: `release_with` passes each mapping's pointer and length as
            // `mmap` returned them, once: a mapping unmapped is removed from its slot.
            &mut |pointer, length| unsafe { v4l2::munmap(pointer, length) },
        )
    }
    /// Stop, unmap and free whatever is still held, through `ioctl`
    /// (STREAMOFF with the buffer type, REQBUFS with a `v4l2_requestbuffers`)
    /// and `unmap`. A device that is gone ([`device_gone`]) has nothing left
    /// to stop or free, so that counts as released; its mappings are still
    /// unmapped, which needs no device.
    fn release_with(
        &mut self,
        ioctl: &mut Ioctl<'_>,
        unmap: &mut dyn FnMut(*mut c_void, usize) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut ioctl = |request, value: *mut c_void| match ioctl(request, value) {
            Err(error) if device_gone(&error) => Ok(()),
            result => result,
        };
        let mut failure = None;
        if self.active {
            let mut kind = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
            match ioctl(
                v4l2::vidioc::VIDIOC_STREAMOFF,
                std::ptr::from_mut(&mut kind).cast(),
            ) {
                Ok(()) => self.active = false,
                Err(error) => {
                    failure = Some(error);
                }
            }
        }
        for slot in &mut self.buffers {
            if let Some(mapping) = slot {
                match unmap(mapping.pointer.as_ptr().cast(), mapping.length) {
                    Ok(()) => *slot = None,
                    Err(error) => {
                        failure.get_or_insert(error);
                    }
                }
            }
        }
        if self.allocated && self.buffers.iter().all(Option::is_none) {
            let mut request = v4l2_requestbuffers {
                count: 0,
                type_: v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE,
                memory: v4l2_memory_V4L2_MEMORY_MMAP,
                // SAFETY: the remaining fields are integers, for which zero is valid
                // (and V4L2 wants reserved fields zero).
                ..unsafe { std::mem::zeroed() }
            };
            match ioctl(
                v4l2::vidioc::VIDIOC_REQBUFS,
                std::ptr::from_mut(&mut request).cast(),
            ) {
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
