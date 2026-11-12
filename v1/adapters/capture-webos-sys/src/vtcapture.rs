//! One VTCapture tap. Lifecycle ported from hyperion-webos.
use crate::ffi::{
    BufferInfo, PlaneInfo, Properties, VtBuffer, VtClient, VtCreate, VtInit, VtPlane, VtPreprocess,
    VtRelease, prysm_vt_buffer, prysm_vt_client, prysm_vt_create, prysm_vt_init, prysm_vt_plane,
    prysm_vt_preprocess, prysm_vt_release,
};
use libloading::Library;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::{self, NonNull};

/// What to capture. LG negotiates exactly these values in `vtCapture_preprocess`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Even, 2–1920.
    pub width: u16,
    /// Even, 2–1080.
    pub height: u16,
    /// 1–60.
    pub fps: u8,
    /// LG dump mode 3 (video blended with menus) instead of 2 (video only).
    pub include_ui: bool,
}

/// The result of one acquisition attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquired {
    /// A packed NV12 frame (limited-range BT.709) of this size is in the output.
    Frame { width: u32, height: u32 },
    /// The producer has no buffer yet (status 2). Poll again.
    Pending,
    /// LG finalized the session (status 1) or it is still initializing; the
    /// next call re-initializes. Callers wait about a second between attempts.
    NotReady,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("cannot load {library}")]
    Load {
        library: String,
        #[source]
        source: libloading::Error,
    },
    #[error("Luna denied the capture registration: {0}")]
    Permission(String),
    /// Driver creation failed; it can leave live threads and stale globals, so
    /// the process must restart before capture is retried.
    #[error("vtCapture_create failed: {0}")]
    Create(Box<Error>),
    #[error("{operation} failed with status {code}")]
    Status { operation: &'static str, code: i32 },
    #[error("vtcapture exception: {0}")]
    Exception(String),
    #[error("invalid native capture: {0}")]
    Invalid(&'static str),
}

impl Error {
    /// Retrying in this process cannot succeed.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Create(_) | Self::Permission(_))
    }
}

macro_rules! api {
    ($name:ident { $($field:ident: $ty:ty = $symbol:literal),* $(,)? }) => {
        struct $name { $($field: $ty,)* _library: Library }
        impl $name {
            fn load(names: &[&str]) -> Result<Self, Error> {
                let library = library(names)?;
                let symbol = |source| Error::Load { library: names.join(" or "), source };
                // SAFETY: signatures and layouts match tv-native-apis. Function
                // pointers are never used after the owning library is dropped.
                unsafe { Ok(Self {
                    $($field: *library.get::<$ty>(concat!($symbol, "\0").as_bytes())
                        .map_err(symbol)?,)*
                    _library: library,
                }) }
            }
        }
    };
}

api!(VtApi {
    create: VtCreate = "vtCapture_create",
    init: VtInit = "vtCapture_init",
    preprocess: VtPreprocess = "vtCapture_preprocess",
    plane: VtPlane = "vtCapture_planeInfo",
    process: VtClient = "vtCapture_process",
    buffer: VtBuffer = "vtCapture_currentCaptureBuffInfo",
    stop: VtClient = "vtCapture_stop",
    postprocess: VtClient = "vtCapture_postprocess",
    finalize: VtClient = "vtCapture_finalize",
    release: VtRelease = "vtCapture_release",
});

fn library(names: &[&str]) -> Result<Library, Error> {
    let mut last = None;
    for name in names {
        // SAFETY: only known LG system libraries are loaded. Their constructors
        // require root and run on the thread that also captures and releases.
        match unsafe { Library::new(*name) } {
            Ok(library) => return Ok(library),
            Err(error) => last = Some(error),
        }
    }
    Err(Error::Load {
        library: names.join(" or "),
        source: last.unwrap_or(libloading::Error::DlOpenUnknown),
    })
}

fn status(code: i32, operation: &'static str) -> Result<(), Error> {
    if code == 0 {
        Ok(())
    } else {
        Err(Error::Status { operation, code })
    }
}

fn vt_result(code: i32, error: &[u8]) -> Result<i32, Error> {
    if code == -1000 {
        return Err(Error::Exception(
            CStr::from_bytes_until_nul(error).map_or_else(
                |_| "unterminated message".into(),
                |m| m.to_string_lossy().into(),
            ),
        ));
    }
    Ok(code)
}

macro_rules! vt_call {
    ($owner:expr, $bridge:ident, $function:expr $(, $arg:expr)* $(,)?) => {{
        let mut error = [0_u8; 512];
        // SAFETY: signatures are statically checked; the exclusively owned
        // driver and borrowed out parameters remain live throughout the call.
        let code = unsafe { $bridge($function, $owner.driver.as_ptr(), $($arg,)* error.as_mut_ptr().cast()) };
        vt_result(code, &error)
    }};
}

/// Borrow a driver plane for copying.
///
/// # Safety
/// The driver must provide a readable allocation of `length` bytes that remains
/// valid and stable until the returned borrow has been copied, before release.
unsafe fn driver_plane<T>(_: &T, pointer: *const u8, length: i32) -> Result<&[u8], Error> {
    let length = usize::try_from(length).map_err(|_| Error::Invalid("negative plane length"))?;
    if pointer.is_null() || length == 0 || length > 64 * 1024 * 1024 {
        return Err(Error::Invalid("driver plane"));
    }
    // SAFETY: caller owns the live native frame; size and nullness checked above.
    Ok(unsafe { std::slice::from_raw_parts(pointer, length) })
}

/// Copy active rows without padding, zero-filling or per-frame allocation.
fn copy_plane(
    source: &[u8],
    stride: usize,
    row_bytes: usize,
    rows: usize,
    output: &mut Vec<u8>,
) -> Result<(), Error> {
    if rows == 0 || rows > 1080 || row_bytes == 0 || stride < row_bytes || stride > 32768 {
        return Err(Error::Invalid("plane geometry"));
    }
    if source.len() < stride * (rows - 1) + row_bytes {
        return Err(Error::Invalid("short plane"));
    }
    output.reserve(row_bytes * rows);
    for row in source.chunks(stride).take(rows) {
        output.extend_from_slice(&row[..row_bytes]);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Created,
    Initialized,
    Prepared,
    Running,
}

/// One native capture tap; see the crate documentation.
pub struct VtCapture {
    api: VtApi,
    driver: NonNull<c_void>,
    client: [c_char; 128],
    stage: Stage,
    plane: PlaneInfo,
    properties: Properties,
    released: bool,
}

/// Prevent the known `LSRegister` denial before allocating vendor video state:
/// the capture role must allow both client names for this process.
fn check_luna() -> Result<(), Error> {
    let pid = std::process::id();
    let names = [
        format!("com.webos.rm.client.prysm.{pid}"),
        format!("com.webos.service.capture.client.prysm.{pid}"),
    ];
    prysm_webos_luna_sys::check_registration(&names).map_err(|error| match error {
        prysm_webos_luna_sys::Error::Load { library, source } => Error::Load { library, source },
        other => Error::Permission(other.to_string()),
    })
}

impl VtCapture {
    /// Load the library, check Luna permission, create the driver and try to
    /// start capture once. A tap that is not ready yet still opens; see
    /// [`Acquired::NotReady`].
    ///
    /// # Errors
    /// Library, permission or native failures; [`Error::is_fatal`] says
    /// whether the process must restart before retrying.
    pub fn open(options: &Options) -> Result<Self, Error> {
        let properties = Properties {
            dump: if options.include_ui { 3 } else { 2 },
            width: i16::try_from(options.width).map_err(|_| Error::Invalid("width"))?,
            height: i16::try_from(options.height).map_err(|_| Error::Invalid("height"))?,
            buffers: 3,
            fps: i32::from(options.fps),
            ..Properties::default()
        };
        let api = VtApi::load(&["libvtcapture.so.1", "libvtcapture.so"])?;
        check_luna()?;
        let mut driver = ptr::null_mut();
        let mut error = [0_u8; 512];
        // SAFETY: typed create symbol and valid out parameters; C++ catches throws.
        let code =
            unsafe { prysm_vt_create(api.create, &raw mut driver, error.as_mut_ptr().cast()) };
        if let Err(error) =
            vt_result(code, &error).and_then(|code| status(code, "vtCapture_create"))
        {
            // Failed construction can leave live threads and stale global pointers.
            std::mem::forget(api);
            return Err(Error::Create(Box::new(error)));
        }
        let driver = NonNull::new(driver).ok_or(Error::Invalid("null driver"))?;
        let mut this = Self {
            api,
            driver,
            client: [0; 128],
            stage: Stage::Created,
            plane: PlaneInfo::default(),
            properties,
            released: false,
        };
        this.client[..2].fill(b'0'.cast_signed());
        this.prepare()?;
        Ok(this)
    }

    /// Initialize, preprocess and start the driver. `Ok(false)`: not ready.
    fn prepare(&mut self) -> Result<bool, Error> {
        if self.stage == Stage::Created {
            let code = vt_call!(
                self,
                prysm_vt_init,
                self.api.init,
                c"prysm-capture-webos".as_ptr(),
                self.client.as_mut_ptr()
            )?;
            if code == 1 {
                return Ok(false);
            }
            status(
                code,
                "vtCapture_init (11 = Luna permission denied, 17 = not ready)",
            )?;
            self.stage = Stage::Initialized;
        }
        let code = vt_call!(
            self,
            prysm_vt_preprocess,
            self.api.preprocess,
            self.client.as_mut_ptr(),
            &raw mut self.properties
        )?;
        if code == 1 {
            // LG finalizes the session on status 1; preprocessing alone cannot reopen it.
            self.stage = Stage::Created;
            return Ok(false);
        }
        status(code, "vtCapture_preprocess")?;
        self.stage = Stage::Prepared;
        let mut plane = PlaneInfo::default();
        status(
            vt_call!(
                self,
                prysm_vt_plane,
                self.api.plane,
                self.client.as_mut_ptr(),
                &raw mut plane
            )?,
            "vtCapture_planeInfo",
        )?;
        let [_, _, width, height] = plane.region;
        if !(2..=1920).contains(&width)
            || !(2..=1080).contains(&height)
            || width % 2 != 0
            || height % 2 != 0
        {
            return Err(Error::Invalid("plane region"));
        }
        if plane.stride < i32::from(width) || plane.stride > 32768 {
            return Err(Error::Invalid("stride"));
        }
        self.plane = plane;
        // A failed process may have partially started the driver; stop on release.
        self.stage = Stage::Running;
        status(
            vt_call!(
                self,
                prysm_vt_client,
                self.api.process,
                self.client.as_mut_ptr()
            )?,
            "vtCapture_process",
        )?;
        Ok(true)
    }

    /// Copy the current frame as packed NV12 into `output` (cleared first, so
    /// a failure never leaves stale pixels).
    ///
    /// # Errors
    /// A native failure; the caller releases this tap and reopens.
    pub fn acquire(&mut self, output: &mut Vec<u8>) -> Result<Acquired, Error> {
        output.clear();
        if self.stage <= Stage::Initialized && !self.prepare()? {
            return Ok(Acquired::NotReady);
        }
        let mut buffer = BufferInfo::default();
        let code = vt_call!(self, prysm_vt_buffer, self.api.buffer, &raw mut buffer)?;
        if code == 2 {
            return Ok(Acquired::Pending);
        }
        status(code, "vtCapture_currentCaptureBuffInfo")?;
        let [_, _, width, height] = self.plane.region;
        let (width, height) = (
            usize::from(width.cast_unsigned()),
            usize::from(height.cast_unsigned()),
        );
        let stride = usize::try_from(self.plane.stride).map_err(|_| Error::Invalid("stride"))?;
        // SAFETY: successful acquisition supplies driver-owned allocations;
        // no further driver call or release occurs before both copies finish.
        let (y, uv) = unsafe {
            (
                driver_plane(self, buffer.y, buffer.y_len)?,
                driver_plane(self, buffer.uv, buffer.uv_len)?,
            )
        };
        let copied = copy_plane(y, stride, width, height, output)
            .and_then(|()| copy_plane(uv, stride, width, height / 2, output));
        if let Err(error) = copied {
            output.clear();
            return Err(error);
        }
        Ok(Acquired::Frame {
            width: width as u32,
            height: height as u32,
        })
    }

    /// Stop, postprocess, finalize and release the driver.
    ///
    /// # Errors
    /// The first cleanup step that failed; all steps still run.
    pub fn release(mut self) -> Result<(), Error> {
        self.cleanup()
    }

    fn cleanup(&mut self) -> Result<(), Error> {
        self.released = true;
        let mut first = None;
        for (needed, function, operation) in [
            (
                self.stage >= Stage::Running,
                self.api.stop,
                "vtCapture_stop",
            ),
            (
                self.stage >= Stage::Prepared,
                self.api.postprocess,
                "vtCapture_postprocess",
            ),
            (
                self.stage >= Stage::Initialized,
                self.api.finalize,
                "vtCapture_finalize",
            ),
        ] {
            if needed {
                let result = vt_call!(self, prysm_vt_client, function, self.client.as_mut_ptr())
                    .and_then(|code| status(code, operation));
                if let Err(error) = result {
                    first.get_or_insert(error);
                }
            }
        }
        if let Err(error) = vt_call!(self, prysm_vt_release, self.api.release) {
            first.get_or_insert(error);
        }
        first.map_or(Ok(()), Err)
    }
}

impl Drop for VtCapture {
    /// Release without reporting when [`VtCapture::release`] was not called.
    fn drop(&mut self) {
        if !self.released {
            let _ = self.cleanup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Driver {
        code: i32,
        initialized: bool,
        initializations: usize,
        y: [u8; 8],
        uv: [u8; 4],
        cleanup: Vec<u8>,
        stop_code: i32,
    }

    fn driver(code: i32) -> Driver {
        Driver {
            code,
            initialized: true,
            initializations: 0,
            y: [16; 8],
            uv: [128; 4],
            cleanup: Vec::new(),
            stop_code: 0,
        }
    }

    unsafe extern "C" fn fake_buffer(driver: *mut c_void, output: *mut BufferInfo) -> i32 {
        // SAFETY: the test owns Driver and BufferInfo for the duration of this call.
        unsafe {
            let driver = &mut *driver.cast::<Driver>();
            if driver.code != 0 {
                return driver.code;
            }
            *output = BufferInfo {
                y: driver.y.as_mut_ptr(),
                uv: driver.uv.as_mut_ptr(),
                y_len: 8,
                uv_len: 4,
            };
        }
        0
    }
    unsafe extern "C" fn fake_stop(driver: *mut c_void, _: *mut c_char) -> i32 {
        // SAFETY: test-owned driver stays alive until after the capture drops.
        let driver = unsafe { &mut *driver.cast::<Driver>() };
        driver.cleanup.push(1);
        driver.stop_code
    }
    unsafe extern "C" fn fake_postprocess(driver: *mut c_void, _: *mut c_char) -> i32 {
        // SAFETY: test-owned driver stays alive until after the capture drops.
        unsafe { (*driver.cast::<Driver>()).cleanup.push(2) };
        0
    }
    unsafe extern "C" fn fake_finalize(driver: *mut c_void, _: *mut c_char) -> i32 {
        // SAFETY: test-owned driver stays alive until after the capture drops.
        unsafe { (*driver.cast::<Driver>()).cleanup.push(3) };
        0
    }
    unsafe extern "C" fn fake_release(driver: *mut c_void) {
        // SAFETY: test owns the driver and checks the event list after drop.
        unsafe { (*driver.cast::<Driver>()).cleanup.push(4) };
    }
    unsafe extern "C" fn unused_create() -> *mut c_void {
        ptr::null_mut()
    }
    unsafe extern "C" fn fake_init(driver: *mut c_void, _: *const c_char, _: *mut c_char) -> i32 {
        // SAFETY: the test owns Driver throughout initialization and capture.
        let driver = unsafe { &mut *driver.cast::<Driver>() };
        driver.initializations += 1;
        driver.initialized = driver.code == 0;
        driver.code
    }
    unsafe extern "C" fn fake_preprocess(
        driver: *mut c_void,
        _: *mut c_char,
        _: *mut Properties,
    ) -> i32 {
        // SAFETY: the test owns the driver until the capture has been dropped.
        let driver = unsafe { &mut *driver.cast::<Driver>() };
        if !driver.initialized {
            return 1;
        }
        // LG preprocessing failure finalizes its session.
        if driver.code != 0 {
            driver.initialized = false;
        }
        driver.code
    }
    unsafe extern "C" fn fake_plane(_: *mut c_void, _: *mut c_char, plane: *mut PlaneInfo) -> i32 {
        // SAFETY: the caller supplies a live output parameter.
        unsafe {
            *plane = PlaneInfo {
                stride: 4,
                region: [0, 0, 2, 2],
                active: [0; 4],
            };
        }
        0
    }
    unsafe extern "C" fn fake_process(_: *mut c_void, _: *mut c_char) -> i32 {
        0
    }

    fn fake_capture(driver: &mut Driver, stage: Stage) -> VtCapture {
        VtCapture {
            api: VtApi {
                create: unused_create,
                init: fake_init,
                preprocess: fake_preprocess,
                plane: fake_plane,
                process: fake_process,
                buffer: fake_buffer,
                stop: fake_stop,
                postprocess: fake_postprocess,
                finalize: fake_finalize,
                release: fake_release,
                _library: libloading::os::unix::Library::this().into(),
            },
            driver: NonNull::from(driver).cast(),
            client: [0; 128],
            stage,
            plane: PlaneInfo {
                stride: 4,
                region: [0, 0, 2, 2],
                active: [0; 4],
            },
            properties: Properties::default(),
            released: false,
        }
    }

    #[test]
    fn finalized_session_reinitializes_without_recreating_the_driver() {
        let mut driver = driver(1);
        let mut capture = fake_capture(&mut driver, Stage::Initialized);
        let mut output = vec![255; 6];
        assert_eq!(capture.acquire(&mut output).unwrap(), Acquired::NotReady);
        assert!(output.is_empty());
        driver.code = 1;
        assert_eq!(capture.acquire(&mut output).unwrap(), Acquired::NotReady);
        assert_eq!(driver.initializations, 1);
        driver.code = 0;
        assert_eq!(
            capture.acquire(&mut output).unwrap(),
            Acquired::Frame {
                width: 2,
                height: 2
            }
        );
        assert_eq!(driver.initializations, 2);
        assert_eq!(output, [16, 16, 16, 16, 128, 128]);
        capture.release().unwrap();
        assert_eq!(driver.cleanup, [1, 2, 3, 4]);
    }

    #[test]
    fn reused_buffers_are_copied_and_errors_never_leave_stale_pixels() {
        let mut driver = driver(0);
        driver.y = [16, 235, 99, 99, 16, 235, 99, 99];
        driver.uv = [128, 128, 99, 99];
        let mut capture = fake_capture(&mut driver, Stage::Running);
        let mut output = Vec::new();
        capture.acquire(&mut output).unwrap();
        assert_eq!(output, [16, 235, 16, 235, 128, 128]);
        driver.y[0] = 235;
        capture.acquire(&mut output).unwrap();
        assert_eq!(output, [235, 235, 16, 235, 128, 128]);
        driver.code = 12;
        assert!(matches!(
            capture.acquire(&mut output),
            Err(Error::Status { code: 12, .. })
        ));
        assert!(output.is_empty());
        drop(capture);
        assert_eq!(driver.cleanup, [1, 2, 3, 4]);
    }

    #[test]
    fn status_two_is_pending_not_an_error() {
        let mut driver = driver(2);
        let mut capture = fake_capture(&mut driver, Stage::Running);
        let mut output = vec![255; 6];
        assert_eq!(capture.acquire(&mut output).unwrap(), Acquired::Pending);
        assert!(output.is_empty());
        driver.code = 0;
        assert!(matches!(
            capture.acquire(&mut output).unwrap(),
            Acquired::Frame { .. }
        ));
    }

    #[test]
    fn release_reports_a_failed_step_after_running_every_step() {
        let mut driver = driver(0);
        driver.stop_code = 5;
        let capture = fake_capture(&mut driver, Stage::Running);
        let error = capture.release().unwrap_err();
        assert_eq!(error.to_string(), "vtCapture_stop failed with status 5");
        assert!(!error.is_fatal());
        assert_eq!(driver.cleanup, [1, 2, 3, 4]);
    }

    #[test]
    fn cleanup_follows_the_stage_reached() {
        let mut driver = driver(0);
        drop(fake_capture(&mut driver, Stage::Created));
        assert_eq!(driver.cleanup, [4]);
    }

    #[test]
    fn missing_library_fails_without_touching_the_driver() {
        let options = Options {
            width: 640,
            height: 360,
            fps: 30,
            include_ui: true,
        };
        if std::path::Path::new("/usr/lib/libvtcapture.so.1").exists() {
            return; // On a TV this would open real hardware; not a host test.
        }
        let Err(error) = VtCapture::open(&options) else {
            panic!("no LG library on the development host");
        };
        assert!(matches!(error, Error::Load { .. }), "{error}");
        assert!(!error.is_fatal());
    }
}
