//! The real backend: LG VTCapture through `prysm-capture-webos-sys`, NV12 to
//! RGB conversion, the cross-process device lock and signal isolation.
use crate::WebosSettings;
use crate::source::{Acquired, Backend, Fault, Tap};
use prysm_capture_webos_sys::{self as sys, VtCapture};
use prysm_core::{Frame, FramePool, PixelFormat};
use prysm_ports::{Error, Result};
use std::fs::File;
use std::path::Path;

/// Shared with the pre-v1 capturer so the two never open the tap together.
const LOCK: &str = "/tmp/prysm-capture.lock";

/// Take the exclusive capture lock. Held until the backend is dropped, which
/// happens only after the last tap was released.
pub(crate) fn capture_lock(path: &Path) -> Result<File> {
    use nix::fcntl::OFlag;
    use std::os::unix::fs::OpenOptionsExt;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        // Root opens a world-writable directory: never follow a planted link.
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(path)
        .map_err(|error| {
            Error::adapter(
                format!("cannot open capture lock {}", path.display()),
                error,
            )
        })?;
    file.try_lock().map_err(|error| {
        Error::adapter("another Prysm capturer owns the TV capture device", error)
    })?;
    Ok(file)
}

/// LG's driver on the TV.
pub(crate) struct Lg {
    _lock: File,
}

impl Lg {
    pub(crate) fn new() -> Result<Self> {
        if !cfg!(all(target_os = "linux", target_arch = "arm")) {
            return Err(Error::other(
                "webOS capture runs only on an LG webOS TV (linux, armv7)",
            ));
        }
        if !nix::unistd::Uid::effective().is_root() {
            return Err(Error::other("webOS capture requires root"));
        }
        Ok(Self {
            _lock: capture_lock(Path::new(LOCK))?,
        })
    }
}

fn fault(context: &str, error: sys::Error) -> Fault {
    Fault {
        fatal: error.is_fatal(),
        error: Error::adapter(context, error),
    }
}

impl Backend for Lg {
    fn open(&mut self, settings: &WebosSettings) -> Result<Box<dyn Tap>, Fault> {
        // A signal handler interrupting LG's preprocessing can corrupt its
        // caller; signals go to the host's other threads instead.
        let mut signals = nix::sys::signal::SigSet::empty();
        signals.add(nix::sys::signal::Signal::SIGINT);
        signals.add(nix::sys::signal::Signal::SIGTERM);
        signals.thread_block().map_err(|error| Fault {
            error: Error::adapter("cannot block signals on the webOS capture thread", error),
            fatal: true,
        })?;
        let capture = VtCapture::open(&settings.options())
            .map_err(|error| fault("cannot open webOS video capture", error))?;
        Ok(Box::new(LgTap {
            capture,
            nv12: Vec::new(),
            pool: FramePool::default(),
        }))
    }
}

struct LgTap {
    capture: VtCapture,
    nv12: Vec<u8>,
    pool: FramePool,
}

impl Tap for LgTap {
    fn acquire(&mut self) -> Result<Acquired, Fault> {
        match self.capture.acquire(&mut self.nv12) {
            Ok(sys::Acquired::Frame { width, height }) => {
                nv12_to_rgb(&mut self.pool, width, height, &self.nv12)
                    .map(Acquired::Frame)
                    .map_err(|error| Fault {
                        error,
                        fatal: false,
                    })
            }
            Ok(sys::Acquired::Pending) => Ok(Acquired::Pending),
            Ok(sys::Acquired::NotReady) => Ok(Acquired::NotReady),
            Err(error) => Err(fault("webOS video interrupted", error)),
        }
    }

    fn release(self: Box<Self>) -> Result<()> {
        self.capture
            .release()
            .map_err(|error| Error::adapter("webOS video cleanup failed", error))
    }
}

/// Packed limited-range BT.709 NV12 to RGB24.
pub(crate) fn nv12_to_rgb(
    pool: &mut FramePool,
    width: u32,
    height: u32,
    nv12: &[u8],
) -> Result<Frame> {
    let luma = width as usize * height as usize;
    if nv12.len() != luma + luma / 2 {
        return Err(Error::other("native NV12 frame has the wrong size"));
    }
    let (y_plane, uv_plane) = nv12.split_at(luma);
    let mut frame = pool
        .acquire(width, height, PixelFormat::RGB24)
        .map_err(|error| Error::adapter("cannot allocate an RGB frame", error))?;
    yuv::yuv_nv12_to_rgb(
        &yuv::YuvBiPlanarImage {
            y_plane,
            y_stride: width,
            uv_plane,
            uv_stride: width,
            width,
            height,
        },
        frame.as_mut_slice(),
        width * 3,
        yuv::YuvRange::Limited,
        yuv::YuvStandardMatrix::Bt709,
        yuv::YuvConversionMode::Balanced,
    )
    .map_err(|error| Error::adapter("cannot convert NV12 to RGB", error))?;
    Ok(frame.finish())
}
