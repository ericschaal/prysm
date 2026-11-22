//! Explicit V4L buffer lifetime. Unlike the upstream stream, cleanup never panics.
use crate::{CaptureError, Result};
use std::{io, ptr::NonNull, sync::Arc, time::Duration};
use v4l::{
    Device,
    device::Handle,
    v4l_sys::{
        v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE, v4l2_buffer, v4l2_memory_V4L2_MEMORY_MMAP,
        v4l2_requestbuffers,
    },
    v4l2,
};

struct Mapping {
    pointer: NonNull<u8>,
    length: usize,
}
pub(super) struct Stream {
    handle: Arc<Handle>,
    buffers: Vec<Option<Mapping>>,
    allocated: bool,
    active: bool,
    timeout: i32,
}

/// Whether `error` from a V4L2 ioctl means the device is gone (unplugged or
/// reset). Once a node is unregistered the V4L2 core answers every ioctl with
/// ENODEV without calling the driver, and unregistering stops streaming and
/// frees the queue's buffers (at the latest when the fd closes). Nothing else
/// counts: EIO, EPROTO or ESHUTDOWN can come from a device still registered,
/// whose buffers may still be held.
pub(super) fn device_gone(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENODEV)
}

/// Whether `error` from `VIDIOC_DQBUF` may be temporary: EIO, which V4L2
/// also uses for problems like signal loss (see [`crate::interruptions`]).
fn interrupted(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::EIO)
}

/// What [`Stream::frame`] got from the device.
pub(super) enum Dequeued<T> {
    /// A buffer, and what was made of it.
    Buffer(T),
    /// No buffer within the timeout.
    Stalled,
    /// DQBUF failed with a possibly temporary error; the stream needs a
    /// restart before the next frame.
    Interrupted(io::Error),
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

impl Stream {
    pub fn new(device: &Device, timeout: Duration) -> Result<Self> {
        let mut stream = Self {
            handle: device.handle(),
            buffers: Vec::new(),
            allocated: false,
            active: false,
            timeout: i32::try_from(timeout.as_millis()).expect("bounded capture timeout"),
        };
        if let Err(error) = stream.allocate() {
            stream
                .release()
                .map_err(|error| CaptureError::Cleanup(error.into()))?;
            return Err(CaptureError::Backend(error.into()));
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
            let pointer = unsafe {
                v4l2::mmap(
                    std::ptr::null_mut(),
                    info.length as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
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
    fn queue(&self, index: u32) -> io::Result<()> {
        // SAFETY: QBUF takes a `v4l2_buffer`.
        unsafe { self.ioctl(v4l2::vidioc::VIDIOC_QBUF, &mut buffer(index)) }
    }
    pub fn start(&mut self) -> io::Result<()> {
        for index in 0..self.buffers.len() {
            self.queue(u32::try_from(index).expect("V4L buffer index"))?;
        }
        let mut kind = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        // SAFETY: STREAMON takes the buffer type, a `u32`.
        unsafe { self.ioctl(v4l2::vidioc::VIDIOC_STREAMON, &mut kind) }?;
        self.active = true;
        Ok(())
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
    pub fn frame<T>(
        &mut self,
        copy: impl FnOnce(&[u8], &v4l2_buffer) -> T,
    ) -> io::Result<Dequeued<T>> {
        if self.handle.poll(libc::POLLIN, self.timeout)? == 0 {
            return Ok(Dequeued::Stalled);
        }
        let mut info = buffer(0);
        // SAFETY: DQBUF takes a `v4l2_buffer`.
        match unsafe { self.ioctl(v4l2::vidioc::VIDIOC_DQBUF, &mut info) } {
            Ok(()) => {}
            // `info` names no buffer: one the driver may have taken comes
            // back only with STREAMOFF.
            Err(error) if interrupted(&error) => return Ok(Dequeued::Interrupted(error)),
            Err(error) => return Err(error),
        }
        let mapping = self
            .buffers
            .get(info.index as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| io::Error::other("capture device returned an invalid buffer index"))?;
        let length = info.bytesused as usize;
        let result = if length > mapping.length {
            Err(io::Error::other("capture frame exceeds its mapped buffer"))
        } else {
            // SAFETY: `pointer` maps `mapping.length >= length` readable bytes
            // (unmapped only by `release`, which needs `&mut self`); the buffer is
            // dequeued, so the driver does not write it until it is queued again
            // below, after `copy` (which cannot keep the slice) returns.
            Ok(copy(
                unsafe { std::slice::from_raw_parts(mapping.pointer.as_ptr(), length) },
                &info,
            ))
        };
        self.queue(info.index)?;
        result.map(Dequeued::Buffer)
    }
    pub fn release(&mut self) -> io::Result<()> {
        let fd = self.handle.fd();
        self.release_with(
            // SAFETY: `release_with` passes only STREAMOFF with the buffer type and
            // REQBUFS with a `v4l2_requestbuffers`, each the structure its request takes.
            |request, value| unsafe { v4l2::ioctl(fd, request, value) },
            // SAFETY: `release_with` passes each mapping's pointer and length as
            // `mmap` returned them, once: a mapping unmapped is removed from its slot.
            |pointer, length| unsafe { v4l2::munmap(pointer, length) },
        )
    }
    /// Stop, unmap and free whatever is still held. A device that is gone
    /// ([`device_gone`]) has nothing left to stop or free, so that counts as
    /// released; its mappings are still unmapped, which needs no device.
    fn release_with(
        &mut self,
        mut ioctl: impl FnMut(v4l2::vidioc::_IOC_TYPE, *mut std::ffi::c_void) -> io::Result<()>,
        mut unmap: impl FnMut(*mut std::ffi::c_void, usize) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut ioctl = |request, value: *mut std::ffi::c_void| match ioctl(request, value) {
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
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn allocated() -> Stream {
        Stream {
            handle: Device::with_path("/dev/null").unwrap().handle(),
            buffers: (0..2)
                .map(|_| {
                    Some(Mapping {
                        pointer: NonNull::dangling(),
                        length: 1,
                    })
                })
                .collect(),
            allocated: true,
            active: true,
            timeout: 0,
        }
    }
    #[test]
    fn every_release_failure_is_retained_and_other_cleanup_is_attempted() {
        for failed_step in ["stop", "unmap", "free"] {
            let mut stream = allocated();
            let calls = RefCell::new(Vec::new());
            let result = stream.release_with(
                |request, _| {
                    let step = if request == v4l2::vidioc::VIDIOC_STREAMOFF {
                        "stop"
                    } else {
                        "free"
                    };
                    calls.borrow_mut().push(step);
                    if step == failed_step {
                        Err(io::Error::other(step))
                    } else {
                        Ok(())
                    }
                },
                |_, _| {
                    calls.borrow_mut().push("unmap");
                    if failed_step == "unmap" {
                        Err(io::Error::other("unmap"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(result.unwrap_err().to_string(), failed_step);
            assert_eq!(stream.active, failed_step == "stop");
            assert_eq!(stream.allocated, failed_step != "stop");
            assert_eq!(
                stream.buffers.iter().filter(|slot| slot.is_some()).count(),
                if failed_step == "unmap" { 2 } else { 0 }
            );
            assert_eq!(
                *calls.borrow(),
                if failed_step == "unmap" {
                    vec!["stop", "unmap", "unmap"]
                } else {
                    vec!["stop", "unmap", "unmap", "free"]
                }
            );
            // A second attempt only touches resources that were not successfully released.
            let retry = RefCell::new(Vec::new());
            stream
                .release_with(
                    |_, _| {
                        retry.borrow_mut().push("ioctl");
                        Ok(())
                    },
                    |_, _| {
                        retry.borrow_mut().push("unmap");
                        Ok(())
                    },
                )
                .unwrap();
            assert!(!stream.active && !stream.allocated);
            assert!(stream.buffers.iter().all(Option::is_none));
            assert_eq!(
                retry.borrow().len(),
                if failed_step == "unmap" { 3 } else { 1 }
            );
        }
    }
    /// Release with `stop` and `free` as the STREAMOFF and REQBUFS results,
    /// counting unmaps.
    fn release_failing(
        stream: &mut Stream,
        stop: Option<i32>,
        free: Option<i32>,
        unmapped: &mut usize,
    ) -> io::Result<()> {
        stream.release_with(
            |request, _| {
                let errno = if request == v4l2::vidioc::VIDIOC_STREAMOFF {
                    stop
                } else {
                    free
                };
                errno.map_or(Ok(()), |errno| Err(io::Error::from_raw_os_error(errno)))
            },
            |_, _| {
                *unmapped += 1;
                Ok(())
            },
        )
    }

    #[test]
    fn a_device_gone_is_released_and_still_unmapped() {
        for (stop, free) in [
            (Some(libc::ENODEV), None),
            (None, Some(libc::ENODEV)),
            (Some(libc::ENODEV), Some(libc::ENODEV)),
        ] {
            let mut stream = allocated();
            let mut unmapped = 0;
            release_failing(&mut stream, stop, free, &mut unmapped).unwrap();
            assert_eq!(unmapped, 2);
            assert!(!stream.active && !stream.allocated);
            assert!(stream.buffers.iter().all(Option::is_none));
            // Nothing is left for a retry (or `Drop`) to touch.
            stream
                .release_with(|_, _| panic!("released"), |_, _| panic!("unmapped"))
                .unwrap();
        }
    }

    #[test]
    fn only_eio_interrupts_a_dequeue() {
        assert!(interrupted(&io::Error::from_raw_os_error(libc::EIO)));
        for errno in [libc::ENODEV, libc::EINVAL, libc::EPIPE] {
            assert!(
                !interrupted(&io::Error::from_raw_os_error(errno)),
                "{errno}"
            );
        }
    }

    #[test]
    fn other_errors_still_fail_the_release() {
        for (stop, free, failed) in [
            (Some(libc::EIO), None, libc::EIO),
            (None, Some(libc::EBUSY), libc::EBUSY),
            // A device gone when stopping does not excuse a failure to free.
            (Some(libc::ENODEV), Some(libc::EBUSY), libc::EBUSY),
        ] {
            let mut stream = allocated();
            let mut unmapped = 0;
            let error = release_failing(&mut stream, stop, free, &mut unmapped).unwrap_err();
            assert_eq!(error.raw_os_error(), Some(failed));
            assert_eq!(unmapped, 2);
            assert_eq!(stream.active, stop == Some(libc::EIO));
            assert_eq!(stream.allocated, free.is_some());
            // Leave nothing for `Drop` to try on the test's fd.
            release_failing(&mut stream, None, None, &mut unmapped).unwrap();
        }
    }

    #[test]
    fn successful_release_is_idempotent_and_preserves_first_error() {
        let mut stream = allocated();
        let error = stream
            .release_with(
                |_, _| Err(io::Error::other("ioctl")),
                |_, _| Err(io::Error::other("unmap")),
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "ioctl");
        stream.release_with(|_, _| Ok(()), |_, _| Ok(())).unwrap();
        stream
            .release_with(
                |_, _| panic!("already released"),
                |_, _| panic!("already unmapped"),
            )
            .unwrap();
    }
}
