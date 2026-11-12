//! The real backend: LG VTCapture and the HAL GAL UI plane through
//! `prysm-capture-webos-sys`, NV12 frames, the cross-process device lock and
//! signal isolation.
use crate::WebosSettings;
use crate::source::{Acquired, Backend, Fault, Tap};
use crate::ui::Nv12Converter;
use prysm_capture_webos_sys::{self as sys, UiCapture, VtCapture};
use prysm_core::{Frame, FramePool, PixelFormat, YuvRange, YuvStandardMatrix};
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

/// A signal handler interrupting LG's native calls can corrupt their caller;
/// signals go to the host's other threads instead.
fn block_signals() -> Result<(), Fault> {
    let mut signals = nix::sys::signal::SigSet::empty();
    signals.add(nix::sys::signal::Signal::SIGINT);
    signals.add(nix::sys::signal::Signal::SIGTERM);
    signals.thread_block().map_err(|error| Fault {
        error: Error::adapter("cannot block signals on the webOS capture thread", error),
        fatal: true,
    })
}

impl Backend for Lg {
    fn open(&mut self, settings: &WebosSettings) -> Result<Box<dyn Tap>, Fault> {
        block_signals()?;
        let capture = VtCapture::open(&settings.options())
            .map_err(|error| fault("cannot open webOS video capture", error))?;
        Ok(Box::new(LgTap {
            capture,
            nv12: Vec::new(),
            pool: FramePool::default(),
        }))
    }

    fn open_ui(&mut self, settings: &WebosSettings) -> Result<Box<dyn Tap>, Fault> {
        block_signals()?;
        let (width, height) = settings.ui_size();
        let capture = UiCapture::open(width, height)
            .map_err(|error| fault("cannot open webOS UI capture", error))?;
        Ok(Box::new(HalTap {
            capture,
            converter: Nv12Converter::default(),
            pool: FramePool::default(),
        }))
    }
}

/// The graphics plane alone, converted to the video tap's NV12.
struct HalTap {
    capture: UiCapture,
    converter: Nv12Converter,
    pool: FramePool,
}

impl Tap for HalTap {
    fn acquire(&mut self) -> Result<Acquired, Fault> {
        let plane = self
            .capture
            .capture()
            .map_err(|error| fault("webOS UI capture failed", error))?;
        let recoverable = |error| Fault {
            error,
            fatal: false,
        };
        let mut frame = self
            .pool
            .acquire(plane.width, plane.height, TAP_FORMAT)
            .map_err(|error| recoverable(Error::adapter("cannot allocate an NV12 frame", error)))?;
        self.converter
            .convert(
                plane.pixels,
                plane.pitch,
                plane.width as usize,
                plane.height as usize,
                frame.as_mut_slice(),
            )
            .map_err(recoverable)?;
        Ok(Acquired::Frame(frame.finish()))
    }

    fn release(self: Box<Self>) -> Result<()> {
        self.capture
            .release()
            .map_err(|error| Error::adapter("webOS UI capture cleanup failed", error))
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
                nv12_frame(&mut self.pool, width, height, &self.nv12)
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

/// What LG's capture tap produces, downstream of HDR tone mapping.
pub(crate) const TAP_FORMAT: PixelFormat = PixelFormat::NV12 {
    range: YuvRange::Limited,
    matrix: YuvStandardMatrix::Bt709,
};

/// The driver's packed NV12 copy as a pooled frame, without colour
/// conversion: one copy into reused storage, no per-frame allocation.
pub(crate) fn nv12_frame(
    pool: &mut FramePool,
    width: u32,
    height: u32,
    nv12: &[u8],
) -> Result<Frame> {
    let expected = TAP_FORMAT
        .expected_size(width, height)
        .map_err(|error| Error::adapter("invalid native NV12 geometry", error))?;
    if expected != Some(nv12.len()) {
        return Err(Error::other("native NV12 frame has the wrong size"));
    }
    let mut frame = pool
        .acquire(width, height, TAP_FORMAT)
        .map_err(|error| Error::adapter("cannot allocate an NV12 frame", error))?;
    frame.as_mut_slice().copy_from_slice(nv12);
    Ok(frame.finish())
}
