//! One VTCapture tap. Lifecycle ported from hyperion-webos.
use crate::ffi::{
    BufferInfo, PlaneInfo, Properties, VtBuffer, VtClient, VtCreate, VtInit, VtPlane, VtPreprocess,
    VtRelease, prysm_vt_buffer, prysm_vt_client, prysm_vt_create, prysm_vt_init, prysm_vt_plane,
    prysm_vt_preprocess, prysm_vt_release,
};
use libloading::Library;
use prysm_webos_luna_sys::{Loaded, load_once};
use std::ffi::{CStr, c_char, c_void};
use std::marker::PhantomData;
use std::ptr::{self, NonNull};
use std::sync::Mutex;

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
#[derive(Debug)]
pub enum Acquired<'a> {
    /// The driver's current buffer, borrowed until it is copied.
    Frame(VideoFrame<'a>),
    /// The producer has no current buffer yet (status 2). Poll again.
    Pending,
    /// The capture device is not available yet, or LG finalized the session
    /// ([`NOT_READY`]); the next call re-initializes. Callers wait about a
    /// second between attempts.
    NotReady,
    /// The library refuses to capture the video the TV shows ([`REFUSED`]).
    /// The driver stays created and the next call initializes again.
    Refused,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot load {library}")]
    Load {
        library: String,
        #[source]
        source: libloading::Error,
    },
    /// The process's role does not allow the capture client names.
    #[error("the capture registration is not allowed")]
    Permission(#[source] prysm_webos_luna_sys::Error),
    /// A capture client name the check registered stays registered, so every
    /// later check in this process fails.
    #[error("a capture client name stays registered")]
    Registered(#[source] prysm_webos_luna_sys::Error),
    /// The registration check failed for another reason. LS2 has no typed
    /// denial, so this may be one worded differently: it is fatal too.
    #[error("the capture registration could not be checked")]
    Luna(#[source] prysm_webos_luna_sys::Error),
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
    /// An earlier driver call of this tap failed; only release is left.
    #[error("the capture tap failed earlier")]
    Failed,
    /// The TV UI capture device is missing or cannot be mapped.
    #[error("cannot use {path}")]
    Device {
        path: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    /// The tap is not reopened: driver creation failed, or the registration
    /// check failed in any way. The source that met it ends; whether a new
    /// source is tried is its host's decision (see [`Self::needs_new_process`]).
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Create(_) | Self::Permission(_) | Self::Registered(_) | Self::Luna(_)
        )
    }

    /// No capture can succeed again in this process: driver creation failed
    /// (it can leave live threads and stale globals), or a client name stays
    /// registered (every later check fails). A new process has new names and
    /// a fresh driver.
    #[must_use]
    pub fn needs_new_process(&self) -> bool {
        matches!(self, Self::Create(_) | Self::Registered(_))
    }
}

/// A library that does not load is [`Error::Load`], a denied name
/// [`Error::Permission`], a name left registered [`Error::Registered`], and
/// any other Luna failure [`Error::Luna`].
impl From<prysm_webos_luna_sys::Error> for Error {
    fn from(error: prysm_webos_luna_sys::Error) -> Self {
        use prysm_webos_luna_sys::Error as Luna;
        match error {
            Luna::Load { library, source } => Self::Load { library, source },
            denied @ Luna::Denied { .. } => Self::Permission(denied),
            unremoved @ Luna::Unremoved { .. } => Self::Registered(unremoved),
            other => Self::Luna(other),
        }
    }
}

/// The length of the message buffer every bridge call writes an exception
/// into, terminated.
const MESSAGE: usize = 512;

macro_rules! api {
    ($name:ident { $($field:ident: $ty:ty = $symbol:literal),* $(,)? }) => {
        /// The VTCapture entry points. Lives for the whole process.
        struct $name { $($field: $ty,)* }
        impl $name {
            fn load(
                library: &'static Library,
                names: &[&str],
            ) -> Result<Self, prysm_webos_luna_sys::Error> {
                let missing = |source| prysm_webos_luna_sys::Error::Load {
                    library: names.join(" or "),
                    source,
                };
                Ok(Self {
                    $($field: {
                        // SAFETY: signatures and layouts match tv-native-apis. The
                        // library is never unloaded, so the pointers stay valid.
                        let symbol = unsafe {
                            library.get::<$ty>(concat!($symbol, "\0").as_bytes())
                        };
                        *symbol.map_err(missing)?
                    },)*
                })
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

/// Load `libvtcapture` on first use and keep it for the life of the process
/// (the vendor code can leave threads running and global pointers set, after
/// a failed create or even after a release).
fn vt() -> Result<&'static VtApi, Error> {
    static VT: Mutex<Loaded<VtApi>> = Mutex::new(Loaded::Nothing);
    let names = ["libvtcapture.so.1", "libvtcapture.so"];
    Ok(load_once(&VT, &names, |library| {
        VtApi::load(library, &names)
    })?)
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
        let mut error = [0_u8; MESSAGE];
        // SAFETY: signatures are statically checked; the exclusively owned
        // driver and borrowed out parameters remain live throughout the call,
        // and the bridge writes at most `MESSAGE` bytes of message.
        let code = unsafe {
            $bridge($function, $owner.driver.as_ptr(), $($arg,)* error.as_mut_ptr().cast(), MESSAGE)
        };
        vt_result(code, &error)
    }};
}

/// Vendor memory: `length` readable bytes at `pointer`, borrowed for `'a`.
/// The vendor side may still write them (a driver reuses its buffers; a
/// graphics blit may not have finished), so they are only ever read through
/// the pointer, never as a Rust slice. Shared by both taps.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Plane<'a> {
    pointer: NonNull<u8>,
    length: usize,
    borrow: PhantomData<&'a [u8]>,
}

impl Plane<'_> {
    /// # Safety
    /// `pointer` must be readable for `length` bytes for the whole lifetime
    /// the plane is given.
    pub(crate) unsafe fn new(pointer: NonNull<u8>, length: usize) -> Self {
        Self {
            pointer,
            length,
            borrow: PhantomData,
        }
    }

    /// Copy the `target.len()` bytes from `offset` into `target`.
    pub(crate) fn copy(&self, offset: usize, target: &mut [u8]) -> Result<(), Error> {
        if offset
            .checked_add(target.len())
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::Invalid("short plane"));
        }
        // SAFETY: `offset + target.len() <= length` (checked above), so the
        // read stays inside the plane, readable for its lifetime (`new`);
        // `target` is Rust memory, apart from the vendor's. Assumed: the only
        // writer is the vendor side, and a write meanwhile may mix frames in
        // the copy.
        unsafe {
            ptr::copy_nonoverlapping(
                self.pointer.as_ptr().add(offset),
                target.as_mut_ptr(),
                target.len(),
            );
        }
        Ok(())
    }
}

/// A driver plane's pointer and length, if they can be one.
fn checked_plane(pointer: *mut u8, length: i32) -> Result<(NonNull<u8>, usize), Error> {
    let length = usize::try_from(length).map_err(|_| Error::Invalid("negative plane length"))?;
    let pointer = NonNull::new(pointer).ok_or(Error::Invalid("driver plane"))?;
    if length == 0 || length > 64 * 1024 * 1024 {
        return Err(Error::Invalid("driver plane"));
    }
    Ok((pointer, length))
}

/// Copy active rows without padding into `output`, exactly `rows * row_bytes` long.
fn copy_plane(
    source: Plane<'_>,
    stride: usize,
    row_bytes: usize,
    rows: usize,
    output: &mut [u8],
) -> Result<(), Error> {
    if rows == 0 || rows > 1080 || row_bytes == 0 || stride < row_bytes || stride > 32768 {
        return Err(Error::Invalid("plane geometry"));
    }
    if output.len() != row_bytes * rows {
        return Err(Error::Invalid("NV12 output size"));
    }
    // A short plane is refused before anything is copied.
    if source.length < stride * (rows - 1) + row_bytes {
        return Err(Error::Invalid("short plane"));
    }
    for (row, target) in output.chunks_exact_mut(row_bytes).enumerate() {
        source.copy(row * stride, target)?;
    }
    Ok(())
}

/// One acquired video frame: the driver's NV12 planes (limited-range BT.709),
/// borrowed from the capture so no driver call can recycle them before
/// [`Self::copy_to`].
#[derive(Debug)]
pub struct VideoFrame<'a> {
    pub width: u32,
    pub height: u32,
    stride: usize,
    y: Plane<'a>,
    uv: Plane<'a>,
}

impl VideoFrame<'_> {
    /// Copy the active rows as packed NV12 into `output`, which must be
    /// exactly the NV12 size of `width` x `height`.
    ///
    /// # Errors
    /// A wrong `output` length or a malformed driver plane; `output` is then
    /// partly written and must not be used.
    pub fn copy_to(&self, output: &mut [u8]) -> Result<(), Error> {
        let (width, height) = (self.width as usize, self.height as usize);
        let (y, uv) = output
            .split_at_mut_checked(width * height)
            .ok_or(Error::Invalid("NV12 output size"))?;
        copy_plane(self.y, self.stride, width, height, y)?;
        copy_plane(self.uv, self.stride, width, height / 2, uv)
    }
}

/// How far the driver got; cleanup undoes each stage reached, last first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Reached {
    Created,
    Initialized,
    Prepared,
    Running,
}

/// Where a tap stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Usable, its driver this far.
    Working(Reached),
    /// A driver call failed with the driver this far: only release is left,
    /// since the driver's state after the failure is unknown.
    Failed(Reached),
    /// Cleaned up.
    Released,
}

/// What [`VtCapture::next`] found: [`Acquired`] before its planes borrow the tap.
enum Next {
    Frame {
        y: (NonNull<u8>, usize),
        uv: (NonNull<u8>, usize),
        width: u32,
        height: u32,
        stride: usize,
    },
    Pending,
    NotReady,
    Refused,
}

/// How far [`VtCapture::prepare`] started the driver.
enum Started {
    Running,
    NotReady,
    Refused,
}

/// The status of `vtCapture_init` when the capture device is not available
/// yet, and of `vtCapture_preprocess` when LG finalized the session.
const NOT_READY: i32 = 1;

/// The status of `vtCapture_init` that refuses the capture of the video the
/// TV shows. Observed on one TV (webOS 10.3.2): AirPlay video got it on every
/// initialization in two sessions, and one YouTube video start in about
/// fifteen got it until the next video. The cause is not known.
const REFUSED: i32 = 17;

/// One native capture tap; see the crate documentation.
pub struct VtCapture {
    api: &'static VtApi,
    driver: NonNull<c_void>,
    /// Boxed, like `properties`: the library gets their addresses and may
    /// keep them, so they must not move with the tap.
    client: Box<[c_char; 128]>,
    stage: Stage,
    plane: PlaneInfo,
    properties: Box<Properties>,
    /// Not `Send`: a tap is opened, used and released on one thread.
    _thread: PhantomData<*const ()>,
}

/// Prevent the known `LSRegister` denial before allocating vendor video state:
/// the capture role must allow both client names for this process.
fn check_luna() -> Result<(), Error> {
    let pid = std::process::id();
    let names = [
        format!("com.webos.rm.client.prysm.{pid}"),
        format!("com.webos.service.capture.client.prysm.{pid}"),
    ];
    Ok(prysm_webos_luna_sys::check_registration(&names)?)
}

impl VtCapture {
    /// Load the library, check Luna permission, create the driver and try to
    /// start capture once. A tap that is not ready yet, or whose video the TV
    /// refuses, still opens; see [`Acquired::NotReady`] and
    /// [`Acquired::Refused`].
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
        let api = vt()?;
        check_luna()?;
        let mut driver = ptr::null_mut();
        let mut error = [0_u8; MESSAGE];
        // SAFETY: typed create symbol and valid out parameters, a message
        // buffer of `MESSAGE` bytes; C++ catches throws.
        let code = unsafe {
            prysm_vt_create(
                api.create,
                &raw mut driver,
                error.as_mut_ptr().cast(),
                MESSAGE,
            )
        };
        vt_result(code, &error)
            .and_then(|code| status(code, "vtCapture_create"))
            .map_err(|error| Error::Create(Box::new(error)))?;
        let driver = NonNull::new(driver).ok_or(Error::Invalid("null driver"))?;
        let mut this = Self {
            api,
            driver,
            client: Box::new([0; 128]),
            stage: Stage::Working(Reached::Created),
            plane: PlaneInfo::default(),
            properties: Box::new(properties),
            _thread: PhantomData,
        };
        // `c_char` is `u8` on ARM (the TV) and `i8` on x86: convert portably.
        this.client[..2].fill(c_char::from_ne_bytes(*b"0"));
        this.step(Self::prepare)?;
        Ok(this)
    }

    /// Runs `step` on a working tap. An error fails the tap, so the next
    /// call is refused and only release is left.
    fn step<T>(&mut self, step: fn(&mut Self) -> Result<T, Error>) -> Result<T, Error> {
        if !matches!(self.stage, Stage::Working(_)) {
            return Err(Error::Failed);
        }
        let result = step(self);
        if let (Err(_), Stage::Working(reached)) = (&result, self.stage) {
            self.stage = Stage::Failed(reached);
        }
        result
    }

    /// Initialize, preprocess and start the driver.
    fn prepare(&mut self) -> Result<Started, Error> {
        if self.stage == Stage::Working(Reached::Created) {
            let code = vt_call!(
                self,
                prysm_vt_init,
                self.api.init,
                c"prysm-capture-webos".as_ptr(),
                self.client.as_mut_ptr()
            )?;
            // Neither initialized the driver, so it stays created: a new
            // driver would only add a release, which can hang (the vendor's
            // lost wake-up in its destructor).
            match code {
                NOT_READY => return Ok(Started::NotReady),
                REFUSED => return Ok(Started::Refused),
                _ => {}
            }
            status(
                code,
                "vtCapture_init (11: on webOS 10.3.2 the library's code suggests the TV's resource manager did not grant the capture resource)",
            )?;
            self.stage = Stage::Working(Reached::Initialized);
        }
        let code = vt_call!(
            self,
            prysm_vt_preprocess,
            self.api.preprocess,
            self.client.as_mut_ptr(),
            &raw mut *self.properties
        )?;
        if code == NOT_READY {
            // LG finalized the session; preprocessing alone cannot reopen it.
            self.stage = Stage::Working(Reached::Created);
            return Ok(Started::NotReady);
        }
        status(code, "vtCapture_preprocess")?;
        self.stage = Stage::Working(Reached::Prepared);
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
        self.stage = Stage::Working(Reached::Running);
        status(
            vt_call!(
                self,
                prysm_vt_client,
                self.api.process,
                self.client.as_mut_ptr()
            )?,
            "vtCapture_process",
        )?;
        Ok(Started::Running)
    }

    /// Borrow the driver's current frame. Its size is known only now: a
    /// re-initialization here can renegotiate it.
    ///
    /// # Errors
    /// A native failure; the tap is then failed: later calls return
    /// [`Error::Failed`], and the caller releases it and reopens.
    pub fn acquire(&mut self) -> Result<Acquired<'_>, Error> {
        Ok(match self.step(Self::next)? {
            Next::Frame {
                y,
                uv,
                width,
                height,
                stride,
            } => {
                // SAFETY: assumed, as nothing documents it: a successful
                // acquisition supplies driver buffers that stay readable
                // until the next driver call. The planes borrow `self` (the
                // returned frame's lifetime), so no driver call or release
                // happens before they are dropped.
                let (y, uv) = unsafe { (Plane::new(y.0, y.1), Plane::new(uv.0, uv.1)) };
                Acquired::Frame(VideoFrame {
                    width,
                    height,
                    stride,
                    y,
                    uv,
                })
            }
            Next::Pending => Acquired::Pending,
            Next::NotReady => Acquired::NotReady,
            Next::Refused => Acquired::Refused,
        })
    }

    /// Prepare the driver if a re-initialization is due, then ask for its
    /// current buffer.
    fn next(&mut self) -> Result<Next, Error> {
        let unprepared = matches!(
            self.stage,
            Stage::Working(Reached::Created | Reached::Initialized)
        );
        if unprepared {
            match self.prepare()? {
                Started::Running => {}
                Started::NotReady => return Ok(Next::NotReady),
                Started::Refused => return Ok(Next::Refused),
            }
        }
        let mut buffer = BufferInfo::default();
        let code = vt_call!(self, prysm_vt_buffer, self.api.buffer, &raw mut buffer)?;
        if code == 2 {
            return Ok(Next::Pending);
        }
        status(code, "vtCapture_currentCaptureBuffInfo")?;
        let [_, _, width, height] = self.plane.region;
        Ok(Next::Frame {
            y: checked_plane(buffer.y, buffer.y_len)?,
            uv: checked_plane(buffer.uv, buffer.uv_len)?,
            width: u32::from(width.cast_unsigned()),
            height: u32::from(height.cast_unsigned()),
            stride: usize::try_from(self.plane.stride).map_err(|_| Error::Invalid("stride"))?,
        })
    }

    /// The driver is created and not initialized: its open or last
    /// acquisition answered [`Acquired::NotReady`] or [`Acquired::Refused`],
    /// so releasing it undoes only its creation, and the next acquisition
    /// initializes it. `false` once a driver call failed.
    #[must_use]
    pub fn created_only(&self) -> bool {
        self.stage == Stage::Working(Reached::Created)
    }

    /// Stop, postprocess, finalize and release the driver.
    ///
    /// # Errors
    /// The first cleanup step that failed; all steps still run.
    pub fn release(mut self) -> Result<(), Error> {
        self.cleanup()
    }

    /// Undo each stage reached, working or failed; nothing once released.
    fn cleanup(&mut self) -> Result<(), Error> {
        let reached = match std::mem::replace(&mut self.stage, Stage::Released) {
            Stage::Working(reached) | Stage::Failed(reached) => reached,
            Stage::Released => return Ok(()),
        };
        let mut first = None;
        for (needed, function, operation) in [
            (reached >= Reached::Running, self.api.stop, "vtCapture_stop"),
            (
                reached >= Reached::Prepared,
                self.api.postprocess,
                "vtCapture_postprocess",
            ),
            (
                reached >= Reached::Initialized,
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
        let _ = self.cleanup();
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
        /// The stride `vtCapture_planeInfo` reports.
        plane_stride: i32,
        /// Calls of `vtCapture_currentCaptureBuffInfo`.
        buffers: usize,
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
            plane_stride: 4,
            buffers: 0,
        }
    }

    unsafe extern "C" fn fake_buffer(driver: *mut c_void, output: *mut BufferInfo) -> i32 {
        // SAFETY: the test owns Driver and BufferInfo for the duration of this call.
        unsafe {
            let driver = &mut *driver.cast::<Driver>();
            driver.buffers += 1;
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
    unsafe extern "C" fn fake_plane(
        driver: *mut c_void,
        _: *mut c_char,
        plane: *mut PlaneInfo,
    ) -> i32 {
        // SAFETY: the test owns the driver; the caller supplies a live output parameter.
        unsafe {
            *plane = PlaneInfo {
                stride: (*driver.cast::<Driver>()).plane_stride,
                region: [0, 0, 2, 2],
                active: [0; 4],
            };
        }
        0
    }
    unsafe extern "C" fn fake_process(_: *mut c_void, _: *mut c_char) -> i32 {
        0
    }

    static FAKE_API: VtApi = VtApi {
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
    };

    /// The fake driver, owned through one raw pointer that the test and its
    /// capture both use, so neither access invalidates the other.
    struct Fake(NonNull<Driver>);

    impl Fake {
        fn new(code: i32) -> Self {
            Self(NonNull::new(Box::into_raw(Box::new(driver(code)))).unwrap())
        }

        /// A capture of this driver; declare it after the `Fake` so it drops first.
        fn capture(&self, reached: Reached) -> VtCapture {
            VtCapture {
                api: &FAKE_API,
                driver: self.0.cast(),
                client: Box::new([0; 128]),
                stage: Stage::Working(reached),
                plane: PlaneInfo {
                    stride: 4,
                    region: [0, 0, 2, 2],
                    active: [0; 4],
                },
                properties: Box::default(),
                _thread: PhantomData,
            }
        }

        /// Read or change the driver between capture calls.
        fn with<R>(&self, change: impl FnOnce(&mut Driver) -> R) -> R {
            // SAFETY: the pointer is live until drop; the capture touches the
            // driver only inside its own calls, which never overlap this one.
            change(unsafe { &mut *self.0.as_ptr() })
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            // SAFETY: allocated by `Box::into_raw` in `new`; its captures were dropped first.
            drop(unsafe { Box::from_raw(self.0.as_ptr()) });
        }
    }

    /// Acquire a frame and copy it as a pooled frame is filled.
    fn copied(capture: &mut VtCapture) -> Vec<u8> {
        let Acquired::Frame(frame) = capture.acquire().unwrap() else {
            panic!("no frame");
        };
        let mut output = vec![0; frame.width as usize * frame.height as usize * 3 / 2];
        frame.copy_to(&mut output).unwrap();
        output
    }

    #[test]
    fn finalized_session_reinitializes_without_recreating_the_driver() {
        let fake = Fake::new(NOT_READY);
        let mut capture = fake.capture(Reached::Initialized);
        assert!(matches!(capture.acquire().unwrap(), Acquired::NotReady));
        assert!(capture.created_only(), "finalized: back to created");
        assert!(matches!(capture.acquire().unwrap(), Acquired::NotReady));
        assert_eq!(fake.with(|driver| driver.initializations), 1);
        fake.with(|driver| driver.code = 0);
        assert_eq!(copied(&mut capture), [16, 16, 16, 16, 128, 128]);
        assert!(!capture.created_only());
        assert_eq!(fake.with(|driver| driver.initializations), 2);
        capture.release().unwrap();
        assert_eq!(fake.with(|driver| driver.cleanup.clone()), [1, 2, 3, 4]);
    }

    /// A refused video keeps the driver: nothing is released, and each call
    /// initializes the same driver again.
    #[test]
    fn refused_video_keeps_the_driver_and_initializes_it_again() {
        let fake = Fake::new(REFUSED);
        let mut capture = fake.capture(Reached::Created);
        assert!(matches!(capture.acquire().unwrap(), Acquired::Refused));
        assert!(matches!(capture.acquire().unwrap(), Acquired::Refused));
        assert!(capture.created_only());
        assert_eq!(fake.with(|driver| driver.initializations), 2);
        assert_eq!(fake.with(|driver| driver.cleanup.clone()), []);
        fake.with(|driver| driver.code = 0);
        assert_eq!(copied(&mut capture), [16, 16, 16, 16, 128, 128]);
        assert!(!capture.created_only());
        assert_eq!(fake.with(|driver| driver.initializations), 3);
        capture.release().unwrap();
        assert_eq!(fake.with(|driver| driver.cleanup.clone()), [1, 2, 3, 4]);
    }

    #[test]
    fn reused_buffers_are_copied_into_exactly_sized_output() {
        let fake = Fake::new(0);
        fake.with(|driver| {
            driver.y = [16, 235, 99, 99, 16, 235, 99, 99];
            driver.uv = [128, 128, 99, 99];
        });
        let mut capture = fake.capture(Reached::Running);
        assert_eq!(copied(&mut capture), [16, 235, 16, 235, 128, 128]);
        fake.with(|driver| driver.y[0] = 235);
        assert_eq!(copied(&mut capture), [235, 235, 16, 235, 128, 128]);
        let Acquired::Frame(frame) = capture.acquire().unwrap() else {
            panic!("no frame");
        };
        assert_eq!((frame.width, frame.height), (2, 2));
        for mut wrong in [vec![0; 3], vec![0; 5], vec![0; 7]] {
            assert!(matches!(
                frame.copy_to(&mut wrong),
                Err(Error::Invalid("NV12 output size"))
            ));
        }
        fake.with(|driver| driver.code = 12);
        assert!(matches!(
            capture.acquire(),
            Err(Error::Status { code: 12, .. })
        ));
        drop(capture);
        assert_eq!(fake.with(|driver| driver.cleanup.clone()), [1, 2, 3, 4]);
    }

    /// Only active rows are read, never past the plane.
    #[test]
    fn planes_are_copied_through_their_pointer_within_bounds() {
        let mut source = [1_u8, 2, 99, 99, 3, 4];
        let (pointer, length) = checked_plane(source.as_mut_ptr(), 6).unwrap();
        // SAFETY: `source` outlives the plane and is not otherwise used meanwhile.
        let plane = unsafe { Plane::new(pointer, length) };
        let mut output = [0; 4];
        copy_plane(plane, 4, 2, 2, &mut output).unwrap();
        assert_eq!(output, [1, 2, 3, 4]);
        assert!(matches!(
            copy_plane(plane, 4, 3, 2, &mut [0; 6]),
            Err(Error::Invalid("short plane"))
        ));
        let mut row = [0; 2];
        plane.copy(4, &mut row).unwrap();
        assert_eq!(row, [3, 4]);
        for offset in [5, usize::MAX] {
            assert!(matches!(
                plane.copy(offset, &mut row),
                Err(Error::Invalid("short plane"))
            ));
        }
        assert!(matches!(
            checked_plane(ptr::null_mut(), 6),
            Err(Error::Invalid("driver plane"))
        ));
    }

    #[test]
    fn a_failed_call_leaves_only_release() {
        // A failed acquisition: the next does not reach the driver.
        let fake = Fake::new(12);
        let mut capture = fake.capture(Reached::Running);
        assert!(matches!(
            capture.acquire(),
            Err(Error::Status { code: 12, .. })
        ));
        fake.with(|driver| driver.code = 0);
        assert!(matches!(capture.acquire(), Err(Error::Failed)));
        capture.release().unwrap();
        assert_eq!(fake.with(|driver| driver.cleanup.clone()), [1, 2, 3, 4]);

        // A failed preparation: the next acquisition neither prepares nor
        // asks for a buffer; release undoes what the driver reached.
        let fake = Fake::new(0);
        fake.with(|driver| driver.plane_stride = 1);
        let mut capture = fake.capture(Reached::Initialized);
        assert!(matches!(capture.acquire(), Err(Error::Invalid("stride"))));
        assert!(matches!(capture.acquire(), Err(Error::Failed)));
        assert_eq!(fake.with(|driver| driver.buffers), 0);
        drop(capture);
        assert_eq!(fake.with(|driver| driver.cleanup.clone()), [2, 3, 4]);
    }

    /// Every stage reached, working or failed: what an acquisition does and
    /// what release undoes.
    #[test]
    fn every_stage_is_released_by_what_it_reached() {
        let undone = |reached| match reached {
            Reached::Created => vec![4],
            Reached::Initialized => vec![3, 4],
            Reached::Prepared => vec![2, 3, 4],
            Reached::Running => vec![1, 2, 3, 4],
        };
        for reached in [
            Reached::Created,
            Reached::Initialized,
            Reached::Prepared,
            Reached::Running,
        ] {
            let fake = Fake::new(0);
            let mut capture = fake.capture(reached);
            capture.stage = Stage::Failed(reached);
            assert!(matches!(capture.acquire(), Err(Error::Failed)));
            assert_eq!(
                fake.with(|driver| (driver.initializations, driver.buffers)),
                (0, 0),
                "{reached:?}: a failed tap never reaches the driver"
            );
            capture.release().unwrap();
            assert_eq!(fake.with(|driver| driver.cleanup.clone()), undone(reached));

            let fake = Fake::new(0);
            drop(fake.capture(reached));
            assert_eq!(
                fake.with(|driver| driver.cleanup.clone()),
                undone(reached),
                "{reached:?}, working"
            );
        }
    }

    #[test]
    fn every_failed_registration_check_is_fatal() {
        use prysm_webos_luna_sys::Error as Luna;
        let denied = Error::from(Luna::Denied {
            name: "com.webos.rm.client.prysm.1".into(),
            message: "Invalid permissions for com.webos.rm.client.prysm.1".into(),
        });
        assert!(matches!(denied, Error::Permission(_)) && denied.is_fatal());
        let unremoved = Error::from(Luna::Unremoved {
            name: "com.webos.rm.client.prysm.1".into(),
            message: "failed".into(),
        });
        assert!(matches!(unremoved, Error::Registered(_)) && unremoved.is_fatal());
        for other in [
            Luna::Luna {
                operation: "LSRegister",
                message: "Service name already registered".into(),
            },
            Luna::Luna {
                operation: "LSErrorInit",
                message: "cannot initialize a Luna error".into(),
            },
            Luna::Nul,
        ] {
            let error = Error::from(other);
            assert!(
                matches!(error, Error::Luna(_)) && error.is_fatal(),
                "{error}"
            );
        }
    }

    #[test]
    fn status_two_is_pending_not_an_error() {
        let fake = Fake::new(2);
        let mut capture = fake.capture(Reached::Running);
        assert!(matches!(capture.acquire().unwrap(), Acquired::Pending));
        fake.with(|driver| driver.code = 0);
        assert!(matches!(capture.acquire().unwrap(), Acquired::Frame(_)));
    }

    #[test]
    fn release_reports_a_failed_step_after_running_every_step() {
        let fake = Fake::new(0);
        fake.with(|driver| driver.stop_code = 5);
        let capture = fake.capture(Reached::Running);
        let error = capture.release().unwrap_err();
        assert_eq!(error.to_string(), "vtCapture_stop failed with status 5");
        assert!(!error.is_fatal());
        assert_eq!(fake.with(|driver| driver.cleanup.clone()), [1, 2, 3, 4]);
    }

    #[test]
    fn cleanup_follows_the_stage_reached() {
        let fake = Fake::new(0);
        drop(fake.capture(Reached::Created));
        assert_eq!(fake.with(|driver| driver.cleanup.clone()), [4]);
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
