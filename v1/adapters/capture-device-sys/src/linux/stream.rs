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

fn buffer(index: u32) -> v4l2_buffer {
    v4l2_buffer {
        index,
        type_: v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE,
        memory: v4l2_memory_V4L2_MEMORY_MMAP,
        // SAFETY: V4L2 buffer fields are plain integers/unions initialized by the ioctl.
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
    fn ioctl<T>(&self, request: v4l2::vidioc::_IOC_TYPE, value: &mut T) -> io::Result<()> {
        // SAFETY: callers pair each ioctl with its matching initialized V4L2 structure.
        unsafe { v4l2::ioctl(self.handle.fd(), request, std::ptr::from_mut(value).cast()) }
    }
    fn request_buffers(&self, count: u32) -> io::Result<u32> {
        let mut request = v4l2_requestbuffers {
            count,
            type_: v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE,
            memory: v4l2_memory_V4L2_MEMORY_MMAP,
            // SAFETY: all reserved fields must be zero for VIDIOC_REQBUFS.
            ..unsafe { std::mem::zeroed() }
        };
        self.ioctl(v4l2::vidioc::VIDIOC_REQBUFS, &mut request)?;
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
            self.ioctl(v4l2::vidioc::VIDIOC_QUERYBUF, &mut info)?;
            if info.length == 0 {
                return Err(io::Error::other("capture device returned an empty buffer"));
            }
            // SAFETY: QUERYBUF supplies this buffer's mmap offset and length for this fd.
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
        self.ioctl(v4l2::vidioc::VIDIOC_QBUF, &mut buffer(index))
    }
    pub fn start(&mut self) -> io::Result<()> {
        for index in 0..self.buffers.len() {
            self.queue(u32::try_from(index).expect("V4L buffer index"))?;
        }
        let mut kind = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        self.ioctl(v4l2::vidioc::VIDIOC_STREAMON, &mut kind)?;
        self.active = true;
        Ok(())
    }
    pub fn stop(&mut self) -> io::Result<()> {
        if self.active {
            let mut kind = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
            self.ioctl(v4l2::vidioc::VIDIOC_STREAMOFF, &mut kind)?;
            self.active = false;
        }
        Ok(())
    }
    pub fn frame<T>(&mut self, copy: impl FnOnce(&[u8], &v4l2_buffer) -> T) -> io::Result<T> {
        if self.handle.poll(libc::POLLIN, self.timeout)? == 0 {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "VIDIOC_DQBUF"));
        }
        let mut info = buffer(0);
        self.ioctl(v4l2::vidioc::VIDIOC_DQBUF, &mut info)?;
        let mapping = self
            .buffers
            .get(info.index as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| io::Error::other("capture device returned an invalid buffer index"))?;
        let length = info.bytesused as usize;
        let result = if length > mapping.length {
            Err(io::Error::other("capture frame exceeds its mapped buffer"))
        } else {
            // SAFETY: this buffer is dequeued and owned by userspace until queue below.
            Ok(copy(
                unsafe { std::slice::from_raw_parts(mapping.pointer.as_ptr(), length) },
                &info,
            ))
        };
        self.queue(info.index)?;
        result
    }
    pub fn release(&mut self) -> io::Result<()> {
        let fd = self.handle.fd();
        self.release_with(
            // SAFETY: release_with supplies only STREAMOFF and REQBUFS with their matching structs.
            |request, value| unsafe { v4l2::ioctl(fd, request, value) },
            // SAFETY: each mapping carries the exact pointer/length returned by mmap.
            |pointer, length| unsafe { v4l2::munmap(pointer, length) },
        )
    }
    fn release_with(
        &mut self,
        mut ioctl: impl FnMut(v4l2::vidioc::_IOC_TYPE, *mut std::ffi::c_void) -> io::Result<()>,
        mut unmap: impl FnMut(*mut std::ffi::c_void, usize) -> io::Result<()>,
    ) -> io::Result<()> {
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
                // SAFETY: reserved fields are required to be zero by V4L2.
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
