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
use crate::vtcapture::{Error, library};
use std::ffi::c_int;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::{Mutex, PoisonError};

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

// The TV's 32-bit ABI, as in halgal.h.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(std::mem::size_of::<Surface>() == 1056);

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
    static HAL: Mutex<Option<&'static HalApi>> = Mutex::new(None);
    let mut slot = HAL.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(api) = *slot {
        return Ok(api);
    }
    let names = ["libhalgal.so.1", "libhalgal.so"];
    let library = library(&names)?;
    let symbol = |source| Error::Load {
        library: names.join(" or "),
        source,
    };
    // SAFETY: the signatures match halgal.h. The library is leaked below, so
    // the function pointers stay valid for the life of the process.
    let api = unsafe {
        HalApi {
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
        }
    };
    // Never unloaded: unloading libhalgal leaks file descriptors.
    std::mem::forget(library);
    let api: &'static HalApi = Box::leak(Box::new(api));
    *slot = Some(api);
    Ok(api)
}

fn status(code: c_int, operation: &'static str) -> Result<(), Error> {
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
    pub pitch: usize,
    pub pixels: &'a [u8],
}

struct Mapping {
    address: NonNull<u8>,
    length: usize,
    /// Closed after unmapping.
    _device: File,
}

/// One open graphics-plane capture surface; see the module documentation.
pub struct UiCapture {
    api: &'static HalApi,
    /// Boxed: HAL keeps no pointer to it, but a stable address costs nothing.
    surface: Box<Surface>,
    created: bool,
    mapping: Option<Mapping>,
    released: bool,
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
        status(unsafe { (api.init)() }, "HAL_GAL_Init")?;
        let mut this = Self {
            api,
            surface: Box::new(Surface::empty()),
            created: false,
            mapping: None,
            released: false,
        };
        // SAFETY: the surface is a live, exclusively owned out parameter.
        let code = unsafe {
            (api.create)(
                u32::from(width),
                u32::from(height),
                0,
                &raw mut *this.surface,
            )
        };
        status(code, "HAL_GAL_CreateSurface")?;
        // From here on dropping `this` destroys the surface.
        this.created = true;
        let length = this.check(width, height)?;
        this.mapping = Some(map(device, this.surface.offset, length)?);
        Ok(this)
    }

    /// The surface HAL created is the one asked for; returns its mapped length.
    fn check(&self, width: u16, height: u16) -> Result<usize, Error> {
        let surface = &self.surface;
        if (surface.width, surface.height) != (width, height) || width == 0 || height == 0 {
            return Err(Error::Invalid("HAL surface size"));
        }
        if surface.bpp != 32 || usize::from(surface.pitch) < usize::from(width) * 4 {
            return Err(Error::Invalid("HAL surface format"));
        }
        let pitch = usize::from(surface.pitch);
        let rows = usize::from(height);
        let length = match surface.property {
            0 => pitch * rows,
            bytes => bytes as usize,
        };
        // Every row's pixels must lie inside the mapping.
        if length < pitch * (rows - 1) + usize::from(width) * 4 || length > 64 * 1024 * 1024 {
            return Err(Error::Invalid("HAL surface length"));
        }
        Ok(length)
    }

    /// Capture the graphics plane into the surface.
    ///
    /// # Errors
    /// A nonzero `HAL_GAL_CaptureFrameBuffer` status.
    pub fn capture(&mut self) -> Result<UiFrame<'_>, Error> {
        // SAFETY: the created surface is live until release.
        let code = unsafe { (self.api.capture)(&raw mut *self.surface) };
        status(code, "HAL_GAL_CaptureFrameBuffer")?;
        let mapping = self
            .mapping
            .as_ref()
            .ok_or(Error::Invalid("unmapped HAL surface"))?;
        // SAFETY: the mapping is `length` readable bytes until it is unmapped,
        // which needs `&mut self`; the capture call above has completed, so
        // HAL does not write the surface while it is borrowed.
        let pixels =
            unsafe { std::slice::from_raw_parts(mapping.address.as_ptr(), mapping.length) };
        Ok(UiFrame {
            width: u32::from(self.surface.width),
            height: u32::from(self.surface.height),
            pitch: usize::from(self.surface.pitch),
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

    fn cleanup(&mut self) -> Result<(), Error> {
        self.released = true;
        let mut first = None;
        if let Some(mapping) = self.mapping.take() {
            // SAFETY: exactly the region `map` returned; no borrow of it can
            // outlive `&mut self`.
            let code = unsafe { libc::munmap(mapping.address.as_ptr().cast(), mapping.length) };
            if code != 0 {
                first = Some(Error::Device {
                    path: DEVICE,
                    source: std::io::Error::last_os_error(),
                });
            }
            drop(mapping);
        }
        if std::mem::take(&mut self.created) {
            // SAFETY: the surface was created by this HAL and not yet destroyed.
            let code = unsafe { (self.api.destroy)(&raw mut *self.surface) };
            if let Err(error) = status(code, "HAL_GAL_DestroySurface") {
                first.get_or_insert(error);
            }
        }
        first.map_or(Ok(()), Err)
    }
}

impl Drop for UiCapture {
    /// Release without reporting when [`UiCapture::release`] was not called.
    fn drop(&mut self) {
        if !self.released {
            let _ = self.cleanup();
        }
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
        assert_eq!(frame.pixels, pixels.as_slice());
        capture.release().unwrap();
        assert_eq!(events(), ["init", "create", "capture", "destroy"]);
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
        assert!(!error.is_fatal());
    }
}
