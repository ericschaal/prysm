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
        timeout: Duration::ZERO,
    }
}

/// A stream of two 4-byte buffers in memory the test owns, standing in for
/// the driver's mappings.
struct Mapped {
    stream: Stream,
    /// Never resized, so the pointers into it stay valid.
    _memory: Vec<[u8; 4]>,
}

impl Mapped {
    fn new() -> Self {
        let mut memory = vec![[1, 2, 3, 4], [5, 6, 7, 8]];
        let mut stream = allocated();
        stream.buffers = memory
            .iter_mut()
            .map(|bytes| {
                Some(Mapping {
                    pointer: NonNull::new(bytes.as_mut_ptr()).unwrap(),
                    length: bytes.len(),
                })
            })
            .collect();
        Self {
            stream,
            _memory: memory,
        }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // The memory is not a mapping: leave nothing for `Stream::drop` to unmap.
        self.stream
            .release_with(&mut |_| Ok(()), &mut forget)
            .unwrap();
    }
}

/// An unmap that lets the mapping go without unmapping anything.
fn forget(_: Mapping) -> std::result::Result<(), (Mapping, io::Error)> {
    Ok(())
}

fn name(request: &Request<'_>) -> &'static str {
    match request {
        Request::Dqbuf(_) => "dequeue",
        Request::Qbuf(_) => "queue",
        Request::Querybuf(_) => "query",
        Request::StreamOn => "on",
        Request::StreamOff => "off",
        Request::Reqbufs(_) => "free",
    }
}

/// A driver that dequeues buffer `index` holding `bytes`, answering each
/// request with the errno `fails` gives for it (none: success), and records
/// the requests it got.
fn driver<'a>(
    calls: &'a RefCell<Vec<String>>,
    index: u32,
    bytes: u32,
    fails: impl Fn(&str) -> Option<i32> + 'a,
) -> impl FnMut(Request<'_>) -> io::Result<()> + 'a {
    move |request| {
        let name = name(&request);
        match request {
            Request::Dqbuf(info) => {
                info.index = index;
                info.bytesused = bytes;
                calls.borrow_mut().push(format!("{name} {}", info.index));
            }
            Request::Qbuf(info) => calls.borrow_mut().push(format!("{name} {}", info.index)),
            _ => calls.borrow_mut().push(name.to_owned()),
        }
        fails(name).map_or(Ok(()), |errno| Err(io::Error::from_raw_os_error(errno)))
    }
}

#[test]
fn each_request_has_its_own_code_and_structure() {
    use v4l2::vidioc::{
        VIDIOC_DQBUF, VIDIOC_QBUF, VIDIOC_QUERYBUF, VIDIOC_REQBUFS, VIDIOC_STREAMOFF,
        VIDIOC_STREAMON,
    };
    let mut kind = 0;
    let kind_at = std::ptr::from_mut(&mut kind).cast::<c_void>();
    let mut info = buffer(0);
    let info_at = std::ptr::from_mut(&mut info).cast::<c_void>();
    let mut request = requested(0);
    let request_at = std::ptr::from_mut(&mut request).cast::<c_void>();
    let got = raw(Request::Reqbufs(&mut request), &mut kind);
    assert_eq!(got, (VIDIOC_REQBUFS, request_at));
    let got = raw(Request::Querybuf(&mut info), &mut kind);
    assert_eq!(got, (VIDIOC_QUERYBUF, info_at));
    let got = raw(Request::Qbuf(&mut info), &mut kind);
    assert_eq!(got, (VIDIOC_QBUF, info_at));
    let got = raw(Request::Dqbuf(&mut info), &mut kind);
    assert_eq!(got, (VIDIOC_DQBUF, info_at));
    let got = raw(Request::StreamOn, &mut kind);
    assert_eq!(got, (VIDIOC_STREAMON, kind_at));
    let got = raw(Request::StreamOff, &mut kind);
    assert_eq!(got, (VIDIOC_STREAMOFF, kind_at));
}

fn ready(_: i32) -> io::Result<i32> {
    Ok(1)
}

fn copied(dequeued: Dequeued<Vec<u8>>) -> Vec<u8> {
    match dequeued {
        Dequeued::Buffer(bytes) => bytes,
        Dequeued::Stalled => panic!("stalled"),
        Dequeued::Interrupted(error) => panic!("interrupted: {error}"),
    }
}

#[test]
fn a_dequeued_buffer_is_copied_then_queued_again() {
    let mut mapped = Mapped::new();
    let calls = RefCell::new(Vec::new());
    let frame = mapped
        .stream
        .frame_with(
            &mut ready,
            &mut driver(&calls, 1, 3, |_| None),
            |data, info| {
                assert_eq!(info.index, 1);
                data.to_vec()
            },
        )
        .unwrap();
    assert_eq!(copied(frame), [5, 6, 7]);
    assert_eq!(*calls.borrow(), ["dequeue 1", "queue 1"]);
}

#[test]
fn an_oversized_buffer_is_queued_again_and_an_unknown_one_is_not() {
    let mut mapped = Mapped::new();
    let calls = RefCell::new(Vec::new());
    let error = mapped
        .stream
        .frame_with(&mut ready, &mut driver(&calls, 0, 5, |_| None), |_, _| {
            panic!("not copied")
        })
        .err()
        .unwrap();
    assert_eq!(
        prysm_ports::chain(&error),
        "error capturing frame: capture frame exceeds its mapped buffer"
    );
    assert_eq!(*calls.borrow(), ["dequeue 0", "queue 0"]);

    calls.borrow_mut().clear();
    let error = mapped
        .stream
        .frame_with(&mut ready, &mut driver(&calls, 2, 1, |_| None), |_, _| {
            panic!("not copied")
        })
        .err()
        .unwrap();
    assert_eq!(
        prysm_ports::chain(&error),
        "error capturing frame: capture device returned an invalid buffer index"
    );
    assert_eq!(
        *calls.borrow(),
        ["dequeue 2"],
        "an unknown index is never queued"
    );
}

#[test]
fn a_signal_during_the_wait_waits_again() {
    let mut mapped = Mapped::new();
    for (answers, buffer) in [
        ([Err(libc::EINTR), Ok(1)], true),
        ([Err(libc::EINTR), Ok(0)], false),
    ] {
        let mut answers = answers.into_iter();
        let mut poll = |_| {
            answers
                .next()
                .expect("polled twice")
                .map_err(io::Error::from_raw_os_error)
        };
        let calls = RefCell::new(Vec::new());
        let got = mapped
            .stream
            .frame_with(&mut poll, &mut driver(&calls, 0, 2, |_| None), |data, _| {
                data.to_vec()
            })
            .unwrap();
        match got {
            Dequeued::Buffer(bytes) => assert!(buffer && bytes == [1, 2]),
            Dequeued::Stalled => assert!(!buffer),
            Dequeued::Interrupted(error) => panic!("interrupted: {error}"),
        }
    }
    let error = mapped
        .stream
        .frame_with(
            &mut |_| Err(io::Error::from_raw_os_error(libc::EBADF)),
            &mut driver(&RefCell::default(), 0, 2, |_| None),
            |_, _| (),
        )
        .err()
        .unwrap();
    assert!(error.to_string().starts_with("error waiting"), "{error}");
}

#[test]
fn eio_from_dequeue_or_requeue_interrupts_and_other_errors_end_capture() {
    let mut mapped = Mapped::new();
    for (failing, errno, outcome) in [
        ("dequeue", libc::EIO, "interrupted: error capturing frame"),
        (
            "queue",
            libc::EIO,
            "interrupted: failed to requeue capture buffer",
        ),
        (
            "queue",
            libc::EINVAL,
            "failed: failed to requeue capture buffer",
        ),
        (
            "dequeue",
            libc::ENODEV,
            "failed: capture device disconnected: error capturing frame",
        ),
    ] {
        let calls = RefCell::new(Vec::new());
        let got = mapped.stream.frame_with(
            &mut ready,
            &mut driver(&calls, 0, 2, |request| {
                (request == failing).then_some(errno)
            }),
            |data, _| data.to_vec(),
        );
        let got = match got {
            Ok(Dequeued::Interrupted(error)) => format!("interrupted: {error}"),
            Err(error) => format!("failed: {error}"),
            Ok(_) => "a buffer".to_owned(),
        };
        assert_eq!(got, outcome, "{failing} {errno}");
    }
}

#[test]
fn a_failed_start_streams_off_so_buffers_can_be_queued_again() {
    for (failing, expected) in [
        (None, vec!["queue 0", "queue 1", "on"]),
        (Some("on"), vec!["queue 0", "queue 1", "on", "off"]),
        (Some("queue"), vec!["queue 0", "off"]),
    ] {
        let mut mapped = Mapped::new();
        mapped.stream.active = false;
        let calls = RefCell::new(Vec::new());
        let result = mapped
            .stream
            .start_with(&mut driver(&calls, 0, 0, |request| {
                (Some(request) == failing).then_some(libc::EIO)
            }));
        assert_eq!(result.is_ok(), failing.is_none());
        assert_eq!(mapped.stream.active, failing.is_none());
        assert_eq!(*calls.borrow(), expected, "{failing:?}");
    }
}

#[test]
fn a_failed_release_keeps_the_capture_error_as_its_cause() {
    let capture = || {
        Err(CaptureError::Backend(
            anyhow::anyhow!("dequeue failed").context("error capturing frame"),
        ))
    };
    let release = || Err(io::Error::other("STREAMOFF failed"));
    assert!(released(Ok(()), Ok(())).is_ok());
    assert!(matches!(
        released(Ok(()), capture()),
        Err(CaptureError::Backend(_))
    ));
    let Err(CaptureError::Cleanup(cleanup)) = released(release(), Ok(())) else {
        panic!("a failed release");
    };
    assert_eq!(cleanup.to_string(), "STREAMOFF failed");
    let Err(CaptureError::Cleanup(cleanup)) = released(release(), capture()) else {
        panic!("a failed release after a failed capture");
    };
    let causes: Vec<String> = cleanup.chain().map(ToString::to_string).collect();
    assert_eq!(causes.first().unwrap(), "STREAMOFF failed");
    assert_eq!(causes.last().unwrap(), "dequeue failed", "{causes:?}");
    assert!(
        cleanup
            .chain()
            .any(|cause| cause.downcast_ref::<CaptureError>().is_some()),
        "the capture error itself is a cause"
    );
}

#[test]
fn every_release_failure_is_retained_and_other_cleanup_is_attempted() {
    for failed_step in ["stop", "unmap", "free"] {
        let mut stream = allocated();
        let calls = RefCell::new(Vec::new());
        let result = stream.release_with(
            &mut |request| {
                let step = if matches!(request, Request::StreamOff) {
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
            &mut |mapping| {
                calls.borrow_mut().push("unmap");
                if failed_step == "unmap" {
                    Err((mapping, io::Error::other("unmap")))
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
        let retry = RefCell::new(Vec::new());
        stream
            .release_with(
                &mut |_| {
                    retry.borrow_mut().push("ioctl");
                    Ok(())
                },
                &mut |_| {
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
        &mut |request| {
            let errno = if matches!(request, Request::StreamOff) {
                stop
            } else {
                free
            };
            errno.map_or(Ok(()), |errno| Err(io::Error::from_raw_os_error(errno)))
        },
        &mut |_| {
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
        stream
            .release_with(&mut |_| panic!("released"), &mut |_| panic!("unmapped"))
            .unwrap();
    }
}

#[test]
fn other_errors_still_fail_the_release() {
    for (stop, free, failed) in [
        (Some(libc::EIO), None, libc::EIO),
        (None, Some(libc::EBUSY), libc::EBUSY),
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
        .release_with(&mut |_| Err(io::Error::other("ioctl")), &mut |mapping| {
            Err((mapping, io::Error::other("unmap")))
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "ioctl");
    stream.release_with(&mut |_| Ok(()), &mut forget).unwrap();
    stream
        .release_with(&mut |_| panic!("already released"), &mut |_| {
            panic!("already unmapped")
        })
        .unwrap();
}
