use super::ffi::{Capability, Framebuffer, Limitation, OutputState, Rect};
use super::{Mapping, copy_mapping, mapping};
use super::{library, status};
use crate::cli::CapturePoint;
use crate::{cli::CaptureOptions, pixels};
use anyhow::{Context, Result, bail, ensure};
use libloading::Library;
use prysm_webos_protocol::{Header, PixelFormat};
use std::fs::OpenOptions;
use std::{
    ffi::c_void,
    ptr::{self, NonNull},
};

api!(DileApi {
    create: unsafe extern "C" fn(u32) -> *mut c_void = "DILE_VT_Create",
    destroy: unsafe extern "C" fn(*mut c_void) = "DILE_VT_Destroy",
    limitation: unsafe extern "C" fn(*mut c_void, *mut Limitation) -> i32 = "DILE_VT_GetVideoFrameOutputDeviceLimitation",
    dump: unsafe extern "C" fn(*mut c_void, i32) -> i32 = "DILE_VT_SetVideoFrameOutputDeviceDumpLocation",
    region: unsafe extern "C" fn(*mut c_void, i32, *mut Rect) -> i32 = "DILE_VT_SetVideoFrameOutputDeviceOutputRegion",
    state: unsafe extern "C" fn(*mut c_void, u32, *mut OutputState) -> i32 = "DILE_VT_SetVideoFrameOutputDeviceState",
    capability: unsafe extern "C" fn(*mut c_void, *mut Capability) -> i32 = "DILE_VT_GetVideoFrameBufferCapability",
    buffers: unsafe extern "C" fn(*mut c_void, *mut Capability, *mut Framebuffer) -> i32 = "DILE_VT_GetAllVideoFrameBufferProperty",
    current: unsafe extern "C" fn(*mut c_void, *mut Framebuffer, *mut u32) -> i32 = "DILE_VT_GetCurrentVideoFrameBufferProperty",
    start: unsafe extern "C" fn(*mut c_void) -> i32 = "DILE_VT_Start",
    stop: unsafe extern "C" fn(*mut c_void) -> i32 = "DILE_VT_Stop",
});

pub struct DileCapture {
    api: DileApi,
    _logging: Library,
    handle: NonNull<c_void>,
    running: bool,
    freeze: bool,
    state: OutputState,
    info: Framebuffer,
    planes: usize,
    mappings: Vec<Mapping>,
    header: Header,
}

impl DileCapture {
    #[allow(
        clippy::too_many_lines,
        reason = "vendor initialization is a single ordered sequence with shared cleanup guards"
    )]
    pub(super) fn open(options: &CaptureOptions) -> Result<Self> {
        // libdile_vt on some TVs omits its DT_NEEDED entry for PmLogLib.
        // SAFETY: known system logging library; keep its symbols globally
        // available until after the DILE library and its resources are dropped.
        let logging: Library = unsafe {
            libloading::os::unix::Library::open(
                Some("libPmLogLib.so.3"),
                libloading::os::unix::RTLD_NOW | libloading::os::unix::RTLD_GLOBAL,
            )?
        }
        .into();
        let api = DileApi::load(&["libdile_vt.so.0", "libdile_vt.so"])?;
        // SAFETY: this thread exclusively owns each live handle and all out parameters.
        let handle = if options.dile_create_ex {
            // SAFETY: optional vendor symbol has the documented native signature.
            let create = unsafe {
                api._library
                    .get::<unsafe extern "C" fn(u32, i32) -> *mut c_void>(b"DILE_VT_CreateEx\0")?
            };
            // SAFETY: no preexisting handle; arguments follow the vendor ABI.
            unsafe { create(0, 1) }
        } else {
            // SAFETY: this thread owns the live native resource and all borrowed out parameters.
            unsafe { (api.create)(0) }
        };
        let owned_handle = NonNull::new(handle).context("DILE_VT_Create failed")?;
        let mut this = Self {
            api,
            _logging: logging,
            handle: owned_handle,
            running: false,
            freeze: !options.no_freeze,
            state: OutputState {
                framerate: 60 / u32::from(options.fps),
                ..OutputState::default()
            },
            info: Framebuffer::default(),
            planes: 0,
            mappings: Vec::new(),
            header: pixels::header(options.width, options.height, PixelFormat::Rgb)?,
        };
        let a = &this.api;
        let mut limit = Limitation::default();
        status(
            // SAFETY: this thread owns the live native resource and all borrowed out parameters.
            unsafe { (a.limitation)(handle, &raw mut limit) },
            "DILE_VT_GetVideoFrameOutputDeviceLimitation",
        )?;
        ensure!(
            options.width >= limit.scale_down_width && options.height >= limit.scale_down_height,
            "DILE_VT minimum capture size is {}x{}",
            limit.scale_down_width,
            limit.scale_down_height
        );
        let mut location = match options.capture_point {
            CapturePoint::Display => 1,
            CapturePoint::ScalerOutput => 0,
            _ => bail!("DILE supports display and scaler-output capture points"),
        };
        // SAFETY: this thread owns the live native resource and all borrowed out parameters.
        if unsafe { (a.dump)(handle, location) } != 0 {
            ensure!(location == 1, "DILE_VT capture point failed");
            location = 0;
            // SAFETY: this thread owns the live native resource and all borrowed out parameters.
            status(unsafe { (a.dump)(handle, location) }, "DILE_VT scaler dump")?;
        }
        let mut rect = Rect {
            width: u16::try_from(options.width)?,
            height: u16::try_from(options.height)?,
            ..Rect::default()
        };
        status(
            // SAFETY: this thread owns the live native resource and all borrowed out parameters.
            unsafe { (a.region)(handle, location, &raw mut rect) },
            "DILE_VT output region",
        )?;
        status(
            // SAFETY: this thread owns the live native resource and all borrowed out parameters.
            unsafe { (a.state)(handle, 0x10, &raw mut this.state) },
            "DILE_VT frame divider",
        )?;
        status(
            // SAFETY: this thread owns the live native resource and all borrowed out parameters.
            unsafe { (a.state)(handle, 2, &raw mut this.state) },
            "DILE_VT unfreeze",
        )?;
        let mut cap = Capability::default();
        status(
            // SAFETY: this thread owns the live native resource and all borrowed out parameters.
            unsafe { (a.capability)(handle, &raw mut cap) },
            "DILE_VT framebuffer capability",
        )?;
        ensure!(
            (1..=16).contains(&cap.buffers) && (1..=4).contains(&cap.planes),
            "invalid DILE_VT buffer count"
        );
        let mut offsets = vec![vec![0_u32; cap.planes as usize]; cap.buffers as usize];
        let mut pointers: Vec<_> = offsets.iter_mut().map(Vec::as_mut_ptr).collect();
        this.info.offsets = pointers.as_mut_ptr();
        status(
            // SAFETY: this thread owns the live native resource and all borrowed out parameters.
            unsafe { (a.buffers)(handle, &raw mut cap, &raw mut this.info) },
            "DILE_VT framebuffer properties",
        )?;
        let format = match this.info.format {
            1 => PixelFormat::Nv12,
            4 => PixelFormat::Nv16,
            9 => PixelFormat::Rgb,
            other => bail!("unsupported DILE_VT pixel format: {other}"),
        };
        let width = if format == PixelFormat::Rgb {
            this.info.stride / 3
        } else {
            this.info.width
        };
        this.header = pixels::header(width, this.info.height, format)?;
        let row_bytes = width * if format == PixelFormat::Rgb { 3 } else { 1 };
        ensure!(
            this.info.stride >= row_bytes && this.info.stride <= 32768,
            "invalid DILE_VT stride"
        );
        this.planes = if format == PixelFormat::Rgb { 1 } else { 2 };
        ensure!(
            cap.planes as usize >= this.planes,
            "missing DILE_VT chroma plane"
        );
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_SYNC)
            .open("/dev/mem")?;
        let size = this.info.stride as usize * this.info.height as usize;
        for buffer in &offsets {
            for offset in buffer.iter().take(this.planes) {
                this.mappings.push(mapping(&file, *offset, size)?);
            }
        }
        this.info.offsets = ptr::null_mut();
        this.running = true;
        // SAFETY: this thread owns the live native resource and all borrowed out parameters.
        status(unsafe { (this.api.start)(handle) }, "DILE_VT_Start")?;
        Ok(this)
    }

    pub(super) fn capture(&mut self, output: &mut Vec<u8>) -> Result<Header> {
        if self.freeze {
            self.state.frozen = 1;
            status(
                // SAFETY: this thread owns the live native resource and all borrowed out parameters.
                unsafe { (self.api.state)(self.handle.as_ptr(), 2, &raw mut self.state) },
                "DILE_VT freeze",
            )?;
        }
        let result = (|| {
            let mut index = 0;
            status(
                // SAFETY: the owned native surface stays live and stable throughout this copy/call.
                unsafe {
                    (self.api.current)(self.handle.as_ptr(), ptr::null_mut(), &raw mut index)
                },
                "DILE_VT current buffer",
            )?;
            let start = index as usize * self.planes;
            let planes = self
                .mappings
                .get(start..start + self.planes)
                .context("invalid DILE_VT buffer index")?;
            for (i, plane) in planes.iter().enumerate() {
                let rows = if i == 1 && self.header.format == PixelFormat::Nv12 {
                    self.header.height as usize / 2
                } else {
                    self.header.height as usize
                };
                let row_bytes = self.header.width as usize
                    * if self.header.format == PixelFormat::Rgb {
                        3
                    } else {
                        1
                    };
                copy_mapping(plane, self.info.stride as usize, row_bytes, rows, output)?;
            }
            Ok(self.header)
        })();
        if self.freeze {
            self.state.frozen = 0;
            status(
                // SAFETY: this thread owns the live native resource and all borrowed out parameters.
                unsafe { (self.api.state)(self.handle.as_ptr(), 2, &raw mut self.state) },
                "DILE_VT unfreeze",
            )?;
        }
        result
    }
}
use std::os::unix::fs::OpenOptionsExt;
impl Drop for DileCapture {
    fn drop(&mut self) {
        // SAFETY: handle is live and owned; mappings drop only after stop/destroy.
        unsafe {
            if self.running {
                let _ = (self.api.stop)(self.handle.as_ptr());
            }
            (self.api.destroy)(self.handle.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Driver {
        events: Vec<&'static str>,
        fail_current: bool,
    }

    unsafe extern "C" fn state(driver: *mut c_void, flags: u32, state: *mut OutputState) -> i32 {
        assert_eq!(flags, 2);
        // SAFETY: the test owns both arguments until the call returns.
        unsafe {
            (*driver.cast::<Driver>())
                .events
                .push(if (*state).frozen == 1 {
                    "freeze"
                } else {
                    "unfreeze"
                });
        }
        0
    }
    unsafe extern "C" fn current(driver: *mut c_void, _: *mut Framebuffer, index: *mut u32) -> i32 {
        // SAFETY: the test owns the driver and capture supplies a live index.
        unsafe {
            (*driver.cast::<Driver>()).events.push("current");
            *index = 0;
            i32::from((*driver.cast::<Driver>()).fail_current)
        }
    }
    unsafe extern "C" fn stop(driver: *mut c_void) -> i32 {
        // SAFETY: the test driver outlives the capture.
        unsafe {
            (*driver.cast::<Driver>()).events.push("stop");
        }
        0
    }
    unsafe extern "C" fn destroy(driver: *mut c_void) {
        // SAFETY: the test driver outlives the capture.
        unsafe {
            (*driver.cast::<Driver>()).events.push("destroy");
        }
    }
    unsafe extern "C" fn create(_: u32) -> *mut c_void {
        ptr::null_mut()
    }
    unsafe extern "C" fn limitation(_: *mut c_void, _: *mut Limitation) -> i32 {
        0
    }
    unsafe extern "C" fn dump(_: *mut c_void, _: i32) -> i32 {
        0
    }
    unsafe extern "C" fn region(_: *mut c_void, _: i32, _: *mut Rect) -> i32 {
        0
    }
    unsafe extern "C" fn capability(_: *mut c_void, _: *mut Capability) -> i32 {
        0
    }
    unsafe extern "C" fn buffers(_: *mut c_void, _: *mut Capability, _: *mut Framebuffer) -> i32 {
        0
    }

    #[test]
    fn acquisition_failures_unfreeze_before_teardown() {
        for fail_current in [true, false] {
            let mut driver = Driver {
                fail_current,
                ..Driver::default()
            };
            let mut capture = DileCapture {
                api: DileApi {
                    create,
                    destroy,
                    limitation,
                    dump,
                    region,
                    state,
                    capability,
                    buffers,
                    current,
                    start: stop,
                    stop,
                    _library: libloading::os::unix::Library::this().into(),
                },
                _logging: libloading::os::unix::Library::this().into(),
                handle: NonNull::from(&mut driver).cast(),
                running: true,
                freeze: true,
                state: OutputState::default(),
                info: Framebuffer::default(),
                planes: 2,
                mappings: Vec::new(),
                header: pixels::header(2, 2, PixelFormat::Nv12).unwrap(),
            };
            let error = capture.capture(&mut Vec::new()).unwrap_err();
            assert!(error.to_string().contains(if fail_current {
                "DILE_VT current buffer"
            } else {
                "invalid DILE_VT buffer index"
            }));
            assert_eq!(driver.events, ["freeze", "current", "unfreeze"]);
            drop(capture);
            assert_eq!(
                driver.events,
                ["freeze", "current", "unfreeze", "stop", "destroy"]
            );
        }
    }
}
