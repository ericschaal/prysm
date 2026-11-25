//! TV UI (graphics plane) capture through LG's HAL GAL (`libhalgal`).
//!
//! The surface layout and the call sequence (init, create surface, capture
//! into it, read it through a `/dev/gfx` mapping, destroy) match the public
//! `halgal.h` used by hyperion-webos (MIT, see `LICENSE.hyperion`). This is
//! independent code that declares the same ABI.
//!
//! `HAL_GAL_Init` binds an EGL context to the calling thread, so open,
//! capture and release must all run on one thread; [`UiCapture`] is not
//! `Send`. The library is loaded once and never unloaded: unloading it leaks
//! file descriptors.
use crate::vtcapture::{Error, Operation, Plane};
use prysm_webos_luna_sys::{Loaded, load_once};
use std::ffi::c_int;
use std::fs::File;
use std::marker::PhantomData;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Mutex;

/// One HAL GAL surface (`HAL_GAL_SURFACE`).
#[repr(C)]
pub(crate) struct Surface {
    /// Offset of the pixels in `/dev/gfx`.
    pub offset: u32,
    pub physical: *mut u32,
    pub pitch: u16,
    pub bpp: u16,
    pub width: u16,
    pub height: u16,
    pub pixel_format: u16,
    pub palette: [u32; 257],
    /// The mapped length in bytes (0: `pitch * height`).
    pub property: u32,
    pub vendor: u32,
}

// The TV's 32-bit ABI for halgal.h's declaration: HAL writes `palette`,
// `property` and `vendor` behind `pixel_format`, so their offsets are pinned
// as well as the size. (A 2-byte `pixel_format` and a 4-byte enum give the
// same offsets.)
#[cfg(target_pointer_width = "32")]
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<Surface>() == 1056);
    assert!(offset_of!(Surface, physical) == 4);
    assert!(offset_of!(Surface, pitch) == 8);
    assert!(offset_of!(Surface, height) == 14);
    assert!(offset_of!(Surface, pixel_format) == 16);
    assert!(offset_of!(Surface, palette) == 20);
    assert!(offset_of!(Surface, property) == 1048);
    assert!(offset_of!(Surface, vendor) == 1052);
};

impl Surface {
    fn empty() -> Self {
        Self {
            offset: 0,
            physical: std::ptr::null_mut(),
            pitch: 0,
            bpp: 0,
            width: 0,
            height: 0,
            pixel_format: 0,
            palette: [0; 257],
            property: 0,
            vendor: 0,
        }
    }
}

pub(crate) type HalInit = unsafe extern "C" fn() -> c_int;
pub(crate) type HalCreate = unsafe extern "C" fn(u32, u32, u32, *mut Surface) -> c_int;
pub(crate) type HalSurfaceCall = unsafe extern "C" fn(*mut Surface) -> c_int;

/// The four HAL GAL entry points. Lives for the whole process.
pub(crate) struct HalApi {
    pub init: HalInit,
    pub create: HalCreate,
    pub destroy: HalSurfaceCall,
    pub capture: HalSurfaceCall,
}

/// Load `libhalgal` on first use and keep it for the life of the process.
fn hal() -> Result<&'static HalApi, Error> {
    static HAL: Mutex<Loaded<HalApi>> = Mutex::new(Loaded::Nothing);
    let names = ["libhalgal.so.1", "libhalgal.so"];
    // Never unloaded, not even when a symbol is missing: unloading libhalgal
    // leaks file descriptors. A retry reuses the loaded library.
    let api = load_once(&HAL, &names, |library| {
        let symbol = |source| prysm_webos_luna_sys::Error::Load {
            library: names.join(" or "),
            source,
        };
        // SAFETY: the signatures match halgal.h. The library is never
        // unloaded, so the function pointers stay valid for the life of the
        // process.
        unsafe {
            Ok(HalApi {
                init: *library.get::<HalInit>(b"HAL_GAL_Init\0").map_err(symbol)?,
                create: *library
                    .get::<HalCreate>(b"HAL_GAL_CreateSurface\0")
                    .map_err(symbol)?,
                destroy: *library
                    .get::<HalSurfaceCall>(b"HAL_GAL_DestroySurface\0")
                    .map_err(symbol)?,
                capture: *library
                    .get::<HalSurfaceCall>(b"HAL_GAL_CaptureFrameBuffer\0")
                    .map_err(symbol)?,
            })
        }
    })?;
    Ok(api)
}

fn status(code: c_int, operation: Operation) -> Result<(), Error> {
    if code == 0 {
        Ok(())
    } else {
        Err(Error::Status { operation, code })
    }
}

/// The graphics plane as captured: premultiplied BGRA rows, `pitch` bytes
/// apart, borrowed from the uncached `/dev/gfx` mapping until the next call.
#[derive(Debug)]
pub struct UiFrame<'a> {
    pub width: u32,
    pub height: u32,
    pitch: usize,
    pixels: Plane<'a>,
}

impl UiFrame<'_> {
    /// Copy row `row` (its `width * 4` bytes, without the pitch's padding)
    /// into `target`, exactly that long. Each row is read once, as the
    /// mapping is uncached.
    ///
    /// # Errors
    /// A row past the last or a `target` of another length.
    pub fn copy_row(&self, row: usize, target: &mut [u8]) -> Result<(), Error> {
        if row >= self.height as usize || target.len() != self.width as usize * 4 {
            return Err(Error::Invalid("UI row"));
        }
        self.pixels.copy(row * self.pitch, target)
    }
}

/// A surface's geometry as checked when it was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Geometry {
    width: u16,
    height: u16,
    pitch: usize,
}

struct Mapping {
    address: NonNull<u8>,
    length: usize,
    /// Closed after unmapping.
    _device: File,
}

/// How far the surface got; cleanup undoes exactly that.
enum Stage {
    /// HAL created the surface; [`UiCapture::open_with`] has not mapped it
    /// yet. Never returned from it.
    Created,
    /// Created and mapped: what [`UiCapture::open_with`] returns.
    Mapped {
        /// What [`UiCapture::capture`] reports: HAL may rewrite the surface
        /// struct on every call, the mapping keeps the geometry checked at
        /// open.
        geometry: Geometry,
        mapping: Mapping,
    },
    /// Unmapped and destroyed.
    Released,
}

/// One open graphics-plane capture surface; see the module documentation.
pub struct UiCapture {
    api: &'static HalApi,
    /// Boxed: HAL keeps no pointer to it, but a stable address costs nothing.
    surface: Box<Surface>,
    stage: Stage,
    /// Not `Send`: HAL GAL binds the tap to the thread that opened it.
    _thread: PhantomData<*const ()>,
}

const DEVICE: &str = "/dev/gfx";

impl UiCapture {
    /// Load the library (once per process), initialize HAL GAL on this
    /// thread, create a `width`×`height` surface and map it.
    ///
    /// # Errors
    /// [`Error::Load`] when the library is missing, [`Error::Device`] when
    /// `/dev/gfx` cannot be opened or mapped, a nonzero HAL status or an
    /// unexpected surface. Nothing stays allocated on error.
    pub fn open(width: u16, height: u16) -> Result<Self, Error> {
        Self::open_with(hal()?, Path::new(DEVICE), width, height)
    }

    pub(crate) fn open_with(
        api: &'static HalApi,
        device: &Path,
        width: u16,
        height: u16,
    ) -> Result<Self, Error> {
        // SAFETY: a typed symbol without arguments.
        status(unsafe { (api.init)() }, Operation::HalInit)?;
        let mut surface = Box::new(Surface::empty());
        // SAFETY: the surface is a live, exclusively owned out parameter.
        let code =
            unsafe { (api.create)(u32::from(width), u32::from(height), 0, &raw mut *surface) };
        status(code, Operation::HalCreateSurface)?;
        // From here on dropping `this` destroys the surface.
        let mut this = Self {
            api,
            surface,
            stage: Stage::Created,
            _thread: PhantomData,
        };
        let length = Self::check(&this.surface, width, height)?;
        let mapping = map(device, this.surface.offset, length)?;
        let geometry = Geometry {
            width,
            height,
            pitch: usize::from(this.surface.pitch),
        };
        this.stage = Stage::Mapped { geometry, mapping };
        Ok(this)
    }

    /// The surface HAL created is the one asked for; returns its mapped length.
    fn check(surface: &Surface, width: u16, height: u16) -> Result<usize, Error> {
        if (surface.width, surface.height) != (width, height) || width == 0 || height == 0 {
            return Err(Error::Invalid("HAL surface size"));
        }
        let pitch = usize::from(surface.pitch);
        let row = usize::from(width) * 4;
        if surface.bpp != 32 || pitch < row {
            return Err(Error::Invalid("HAL surface format"));
        }
        let rows = usize::from(height);
        let length = match surface.property {
            0 => pitch.checked_mul(rows),
            bytes => usize::try_from(bytes).ok(),
        };
        // Every row's pixels must lie inside the mapping; `rows` is at least 1.
        let end = pitch
            .checked_mul(rows - 1)
            .and_then(|last| last.checked_add(row));
        match (length, end) {
            (Some(length), Some(end)) if end <= length && length <= 64 * 1024 * 1024 => Ok(length),
            _ => Err(Error::Invalid("HAL surface length")),
        }
    }

    /// Capture the graphics plane into the surface.
    ///
    /// # Errors
    /// A nonzero `HAL_GAL_CaptureFrameBuffer` status.
    pub fn capture(&mut self) -> Result<UiFrame<'_>, Error> {
        // A value outside `open_with` is always mapped: `Created` exists only
        // inside it and `Released` only once `release` has taken the value.
        let Stage::Mapped { geometry, mapping } = &self.stage else {
            return Err(Error::Invalid("unmapped HAL surface"));
        };
        // SAFETY: the created surface is live until release.
        let code = unsafe { (self.api.capture)(&raw mut *self.surface) };
        status(code, Operation::HalCaptureFrameBuffer)?;
        // SAFETY: the mapping is `length` readable bytes until it is unmapped,
        // which needs `&mut self`, so not while the frame borrows it. HAL may
        // still be writing it (the capture is a blit, and nothing says it has
        // finished when the call returns), so it is read only through the
        // plane's pointer.
        let pixels = unsafe { Plane::new(mapping.address, mapping.length) };
        Ok(UiFrame {
            width: u32::from(geometry.width),
            height: u32::from(geometry.height),
            pitch: geometry.pitch,
            pixels,
        })
    }

    /// Unmap, close `/dev/gfx` and destroy the surface.
    ///
    /// # Errors
    /// The first step that failed; all steps still run.
    pub fn release(mut self) -> Result<(), Error> {
        self.cleanup()
    }

    /// Undo the stage reached, once: a second call finds `Released`.
    fn cleanup(&mut self) -> Result<(), Error> {
        let mut first = None;
        match std::mem::replace(&mut self.stage, Stage::Released) {
            Stage::Released => return Ok(()),
            Stage::Created => {}
            Stage::Mapped { mapping, .. } => {
                // SAFETY: exactly the region `map` returned; no borrow of it
                // can outlive `&mut self`.
                let code = unsafe { libc::munmap(mapping.address.as_ptr().cast(), mapping.length) };
                if code != 0 {
                    first = Some(Error::Device {
                        path: DEVICE,
                        source: std::io::Error::last_os_error(),
                    });
                }
                drop(mapping);
            }
        }
        // SAFETY: the surface was created by this HAL and not yet destroyed:
        // the stage was `Created` or `Mapped` and is now `Released`.
        let code = unsafe { (self.api.destroy)(&raw mut *self.surface) };
        if let Err(error) = status(code, Operation::HalDestroySurface) {
            first.get_or_insert(error);
        }
        first.map_or(Ok(()), Err)
    }
}

impl Drop for UiCapture {
    /// Release without reporting; nothing when [`UiCapture::release`] ran.
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Map `length` bytes of `device` at `offset`, read and write as LG's
/// driver expects.
fn map(device: &Path, offset: u32, length: usize) -> Result<Mapping, Error> {
    let error = |source| Error::Device {
        path: DEVICE,
        source,
    };
    let file = File::options()
        .read(true)
        .write(true)
        .open(device)
        .map_err(error)?;
    let (protection, flags) = (libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED);
    // SAFETY: a fresh mapping of an open descriptor; the kernel validates
    // offset and length. The 32-bit offset is never sign-extended.
    let address = unsafe {
        #[cfg(target_os = "linux")]
        let address = libc::mmap64(
            std::ptr::null_mut(),
            length,
            protection,
            flags,
            file.as_raw_fd(),
            i64::from(offset),
        );
        #[cfg(not(target_os = "linux"))]
        let address = libc::mmap(
            std::ptr::null_mut(),
            length,
            protection,
            flags,
            file.as_raw_fd(),
            libc::off_t::from(offset),
        );
        address
    };
    if address == libc::MAP_FAILED {
        return Err(error(std::io::Error::last_os_error()));
    }
    Ok(Mapping {
        address: NonNull::new(address.cast()).ok_or(Error::Invalid("null mapping"))?,
        length,
        _device: file,
    })
}

/// The fuzz target `halgal_surface` in `fuzz/` (feature `fuzzing`, never
/// shipped): [`UiCapture::check`] on any surface HAL could hand back, read
/// from 16 bytes, against the same rules worked out in `u64`. `check` accepts
/// exactly the surfaces whose every row lies inside a mapping of at most
/// 64 MiB, and returns that mapping's length.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub fn fuzz_surface_check(data: &[u8]) {
    let Some(bytes) = data.first_chunk::<16>() else {
        return;
    };
    let field = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let (width, height) = (field(0), field(2));
    let mut surface = Surface::empty();
    surface.pitch = field(4);
    surface.bpp = field(6);
    surface.width = field(8);
    surface.height = field(10);
    surface.property = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let s = &surface;
    let (pitch, rows, row) = (u64::from(s.pitch), u64::from(height), u64::from(width) * 4);
    let length = match s.property {
        0 => pitch * rows,
        bytes => u64::from(bytes),
    };
    let valid = (s.width, s.height) == (width, height)
        && width != 0
        && height != 0
        && s.bpp == 32
        && pitch >= row
        && length >= pitch * (rows - 1) + row
        && length <= 64 * 1024 * 1024;
    match UiCapture::check(s, width, height) {
        Ok(got) => {
            assert!(valid, "accepted {width}x{height} from {data:?}");
            assert_eq!(got as u64, length, "{data:?}");
        }
        Err(Error::Invalid(_)) => assert!(!valid, "refused {width}x{height} from {data:?}"),
        Err(error) => panic!("{error} from {data:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;
    use std::thread::ThreadId;

    // The fakes have no context argument; each test runs on its own thread,
    // so thread-local state also proves every call happened on that thread.
    thread_local! {
        static EVENTS: RefCell<Vec<(&'static str, ThreadId)>> = const { RefCell::new(Vec::new()) };
        static FAIL: RefCell<Option<&'static str>> = const { RefCell::new(None) };
        static PITCH: RefCell<u16> = const { RefCell::new(0) };
    }

    fn event(name: &'static str) -> c_int {
        EVENTS.with(|e| e.borrow_mut().push((name, std::thread::current().id())));
        if FAIL.with(|f| *f.borrow() == Some(name)) {
            -1
        } else {
            0
        }
    }

    fn events() -> Vec<&'static str> {
        EVENTS.with(|e| {
            let events = e.borrow();
            let me = std::thread::current().id();
            assert!(events.iter().all(|(_, thread)| *thread == me));
            events.iter().map(|(name, _)| *name).collect()
        })
    }

    unsafe extern "C" fn fake_init() -> c_int {
        event("init")
    }
    unsafe extern "C" fn fake_create(w: u32, h: u32, _: u32, surface: *mut Surface) -> c_int {
        let code = event("create");
        if code == 0 {
            let pitch = PITCH.with(|p| *p.borrow());
            // SAFETY: the caller passes a live, exclusive surface.
            let surface = unsafe { &mut *surface };
            surface.width = w as u16;
            surface.height = h as u16;
            surface.pitch = pitch;
            surface.bpp = 32;
            surface.property = u32::from(pitch) * h;
        }
        code
    }
    unsafe extern "C" fn fake_capture(_: *mut Surface) -> c_int {
        event("capture")
    }
    unsafe extern "C" fn fake_destroy(_: *mut Surface) -> c_int {
        event("destroy")
    }

    fn api() -> &'static HalApi {
        static API: HalApi = HalApi {
            init: fake_init,
            create: fake_create,
            destroy: fake_destroy,
            capture: fake_capture,
        };
        &API
    }

    /// A stand-in for `/dev/gfx` holding a 2×2 surface with an 8-byte pitch
    /// pad, so rows are `pitch` apart.
    fn device(pixels: &[u8]) -> tempfile::NamedTempFile {
        PITCH.with(|p| *p.borrow_mut() = 16);
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(pixels).unwrap();
        file.flush().unwrap();
        file
    }

    #[test]
    fn open_capture_release_run_in_order_on_one_thread() {
        let pixels: Vec<u8> = (0..32).collect();
        let file = device(&pixels);
        let mut capture = UiCapture::open_with(api(), file.path(), 2, 2).unwrap();
        let frame = capture.capture().unwrap();
        assert_eq!((frame.width, frame.height, frame.pitch), (2, 2, 16));
        let mut row = [0; 8];
        for (index, start) in [(0, 0), (1, 16)] {
            frame.copy_row(index, &mut row).unwrap();
            assert_eq!(row, pixels[start..start + 8], "row {index}");
        }
        assert!(matches!(
            frame.copy_row(2, &mut row),
            Err(Error::Invalid("UI row"))
        ));
        assert!(matches!(
            frame.copy_row(0, &mut [0; 7]),
            Err(Error::Invalid("UI row"))
        ));
        capture.release().unwrap();
        assert_eq!(events(), ["init", "create", "capture", "destroy"]);
    }

    #[test]
    fn a_capture_reports_the_geometry_checked_at_open() {
        let file = device(&[0; 32]);
        let mut capture = UiCapture::open_with(api(), file.path(), 2, 2).unwrap();
        // HAL rewrites the struct it is handed; the frame keeps to the mapping.
        capture.surface.height = 4000;
        capture.surface.pitch = 4000;
        let frame = capture.capture().unwrap();
        assert_eq!((frame.width, frame.height, frame.pitch), (2, 2, 16));
    }

    #[test]
    fn failed_capture_is_a_status_and_drop_still_destroys() {
        let file = device(&[0; 32]);
        FAIL.with(|f| *f.borrow_mut() = Some("capture"));
        let mut capture = UiCapture::open_with(api(), file.path(), 2, 2).unwrap();
        let error = capture.capture().unwrap_err();
        assert_eq!(
            error.to_string(),
            "HAL_GAL_CaptureFrameBuffer failed with status -1"
        );
        drop(capture);
        assert_eq!(events(), ["init", "create", "capture", "destroy"]);
    }

    #[test]
    fn missing_device_destroys_the_created_surface() {
        PITCH.with(|p| *p.borrow_mut() = 8);
        let error = UiCapture::open_with(api(), Path::new("/nonexistent/gfx"), 2, 2)
            .err()
            .unwrap();
        assert!(matches!(error, Error::Device { .. }), "{error}");
        assert_eq!(events(), ["init", "create", "destroy"]);
    }

    #[test]
    fn failed_creation_destroys_nothing() {
        FAIL.with(|f| *f.borrow_mut() = Some("create"));
        let error = UiCapture::open_with(api(), Path::new("/nonexistent/gfx"), 2, 2)
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "HAL_GAL_CreateSurface failed with status -1"
        );
        assert_eq!(events(), ["init", "create"]);
    }

    #[test]
    fn an_unexpected_surface_is_rejected() {
        PITCH.with(|p| *p.borrow_mut() = 4); // narrower than 2 BGRA pixels
        let error = UiCapture::open_with(api(), Path::new("/nonexistent/gfx"), 2, 2)
            .err()
            .unwrap();
        assert!(matches!(error, Error::Invalid(_)), "{error}");
        assert_eq!(events(), ["init", "create", "destroy"]);
    }

    #[test]
    fn missing_library_is_a_load_error() {
        if Path::new("/dev/gfx").exists() {
            return; // On a TV this would open real hardware; not a host test.
        }
        let error = UiCapture::open(320, 180).err().unwrap();
        assert!(matches!(error, Error::Load { .. }), "{error}");
    }
}
