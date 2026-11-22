//! The real backend: LG VTCapture and the HAL GAL UI plane through
//! `prysm-capture-webos-sys`, NV12 frames, the cross-process device lock and
//! signal isolation.
use crate::WebosSettings;
use crate::source::{Acquired, Backend, Fault, Recovery, Tap};
use crate::ui::Nv12Converter;
use prysm_capture_webos_sys::{self as sys, UiCapture, VtCapture};
use prysm_core::{FramePool, PixelFormat, YuvRange, YuvStandardMatrix};
use prysm_ports::{Error, Result};
use std::fs::File;
use std::path::Path;

/// Held by the worker that owns the TV capture, so a successive worker (or
/// one still finishing after its supervisor let it go) never opens a tap
/// while another holds one.
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

/// A native fault, fatal as [`sys::Error::is_fatal`] says.
pub(crate) fn fault(context: &str, error: sys::Error) -> Fault {
    Fault {
        recovery: if error.is_fatal() {
            Recovery::Never
        } else {
            Recovery::Reopen
        },
        error: Error::adapter(context, error),
    }
}

/// A TV UI open that cannot heal in this process (the library or a symbol
/// is missing, or `/dev/gfx` is) is fatal; any other failure is retried.
pub(crate) fn ui_open_fault(error: sys::Error) -> Fault {
    let fatal = match &error {
        sys::Error::Load { .. } => true,
        sys::Error::Device { source, .. } => source.kind() == std::io::ErrorKind::NotFound,
        _ => false,
    };
    Fault {
        recovery: if fatal {
            Recovery::Never
        } else {
            Recovery::Reopen
        },
        error: Error::adapter("cannot open webOS UI capture", error),
    }
}

/// A signal handler interrupting LG's native calls can corrupt their caller;
/// signals go to the host's other threads instead.
fn block_signals() -> Result<(), Fault> {
    let mut signals = nix::sys::signal::SigSet::empty();
    signals.add(nix::sys::signal::Signal::SIGINT);
    signals.add(nix::sys::signal::Signal::SIGTERM);
    signals.thread_block().map_err(|error| {
        Fault::fatal(Error::adapter(
            "cannot block signals on the webOS capture thread",
            error,
        ))
    })
}

impl Backend for Lg {
    fn open(&mut self, settings: &WebosSettings) -> Result<Box<dyn Tap>, Fault> {
        block_signals()?;
        let capture = VtCapture::open(&settings.options())
            .map_err(|error| fault("cannot open webOS video capture", error))?;
        Ok(Box::new(LgTap {
            capture,
            pool: FramePool::default(),
        }))
    }

    fn open_ui(&mut self, settings: &WebosSettings) -> Result<Box<dyn Tap>, Fault> {
        block_signals()?;
        let (width, height) = settings.ui_size();
        // HAL GAL binds the tap to this thread; the kernel log names a held
        // lock by thread id (`K_pid`).
        tracing::info!(thread = os_thread(), "opening the TV UI tap");
        let capture = UiCapture::open(width, height).map_err(ui_open_fault)?;
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
        let mut frame = self
            .pool
            .acquire(plane.width, plane.height, TAP_FORMAT)
            .map_err(|error| {
                Fault::recoverable(Error::adapter("cannot allocate an NV12 frame", error))
            })?;
        self.converter
            .convert(
                |y, row| {
                    plane
                        .copy_row(y, row)
                        .map_err(|error| Error::adapter("cannot read the TV UI plane", error))
                },
                plane.width as usize,
                plane.height as usize,
                frame.as_mut_slice(),
            )
            .map_err(Fault::recoverable)?;
        Ok(Acquired::Frame(frame.finish()))
    }

    fn release(self: Box<Self>) -> Result<()> {
        // Logged before as well: a release that hangs never logs after.
        tracing::info!(thread = os_thread(), "releasing the TV UI tap");
        self.capture
            .release()
            .map_err(|error| Error::adapter("webOS UI capture cleanup failed", error))?;
        tracing::info!(thread = os_thread(), "TV UI tap released");
        Ok(())
    }
}

/// The kernel's id of the calling thread (0 off Linux).
fn os_thread() -> i32 {
    #[cfg(target_os = "linux")]
    return nix::unistd::gettid().as_raw();
    #[cfg(not(target_os = "linux"))]
    0
}

/// The driver's packed NV12, forwarded without colour conversion: one copy
/// from the driver into reused storage, no per-frame allocation.
struct LgTap {
    capture: VtCapture,
    pool: FramePool,
}

impl Tap for LgTap {
    fn acquire(&mut self) -> Result<Acquired, Fault> {
        let video = match self.capture.acquire() {
            Ok(sys::Acquired::Frame(video)) => video,
            Ok(sys::Acquired::Pending) => return Ok(Acquired::Pending),
            Ok(sys::Acquired::NotReady) => return Ok(Acquired::NotReady),
            Err(error) => return Err(fault("webOS video interrupted", error)),
        };
        // An unfinished frame publishes nothing and goes back to the pool.
        let mut frame = self
            .pool
            .acquire(video.width, video.height, TAP_FORMAT)
            .map_err(|error| {
                Fault::recoverable(Error::adapter("cannot allocate an NV12 frame", error))
            })?;
        video
            .copy_to(frame.as_mut_slice())
            .map_err(|error| fault("webOS video interrupted", error))?;
        Ok(Acquired::Frame(frame.finish()))
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
