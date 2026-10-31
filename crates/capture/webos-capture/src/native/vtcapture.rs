use super::driver_plane;
use super::ffi::{
    BufferInfo, PlaneInfo, Properties, VtBuffer, VtClient, VtCreate, VtInit, VtPlane, VtPreprocess,
    VtRelease, prysm_vt_buffer, prysm_vt_client, prysm_vt_create, prysm_vt_init, prysm_vt_plane,
    prysm_vt_preprocess, prysm_vt_release,
};
use super::{library, status};
use crate::{cli::CaptureOptions, pixels};
use anyhow::{Context, Result, ensure};
use libloading::Library;
use prysm_webos_protocol::{Header, PixelFormat};
use std::{ffi::CStr, fs::OpenOptions, os::fd::AsRawFd};
use std::{
    ffi::{c_char, c_void},
    ptr::{self, NonNull},
    time::{Duration, Instant},
};

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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Created,
    Initialized,
    Prepared,
    Running,
}

pub struct VtCapture {
    api: VtApi,
    driver: NonNull<c_void>,
    client: [c_char; 128],
    stage: Stage,
    plane: PlaneInfo,
    force_capture: bool,
    pending_since: Option<Instant>,
}

fn vt_result(code: i32, error: &[u8]) -> Result<i32> {
    ensure!(
        code != -1000,
        "vtcapture exception: {}",
        CStr::from_bytes_until_nul(error)?.to_string_lossy()
    );
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

impl VtCapture {
    pub(super) fn open(options: &CaptureOptions) -> Result<Self> {
        let api = VtApi::load(&["libvtcapture.so.1", "libvtcapture.so"])?;
        let mut driver = ptr::null_mut();
        let mut error = [0_u8; 512];
        // SAFETY: typed create symbol and valid out parameters; C++ contains throws.
        let code =
            unsafe { prysm_vt_create(api.create, &raw mut driver, error.as_mut_ptr().cast()) };
        status(vt_result(code, &error)?, "vtCapture_create")?;
        let driver = NonNull::new(driver).context("vtcapture returned a null driver")?;
        let mut this = Self {
            api,
            driver,
            client: [0; 128],
            stage: Stage::Created,
            plane: PlaneInfo::default(),
            force_capture: options.force_capture,
            pending_since: None,
        };
        this.client[..2].fill(c_char::try_from(b'0')?);
        let code = vt_call!(
            this,
            prysm_vt_init,
            this.api.init,
            c"prysm-webos-capture".as_ptr(),
            this.client.as_mut_ptr()
        )?;
        if code == 17 && this.force_capture {
            force_capture()?;
        }
        status(
            code,
            "vtCapture_init (11 = Luna permission denied, 17 = not ready)",
        )?;
        this.stage = Stage::Initialized;
        let mut props = Properties {
            dump: options.capture_point as i32,
            width: i16::try_from(options.width)?,
            height: i16::try_from(options.height)?,
            buffers: 3,
            fps: i32::from(options.fps),
            ..Properties::default()
        };
        status(
            vt_call!(
                this,
                prysm_vt_preprocess,
                this.api.preprocess,
                this.client.as_mut_ptr(),
                &raw mut props
            )?,
            "vtCapture_preprocess",
        )?;
        this.stage = Stage::Prepared;
        let mut plane = PlaneInfo::default();
        status(
            vt_call!(
                this,
                prysm_vt_plane,
                this.api.plane,
                this.client.as_mut_ptr(),
                &raw mut plane
            )?,
            "vtCapture_planeInfo",
        )?;
        pixels::header(
            u32::try_from(plane.region[2])?,
            u32::try_from(plane.region[3])?,
            PixelFormat::Nv12,
        )?;
        ensure!(
            plane.stride >= i32::from(plane.region[2]) && plane.stride <= 32768,
            "invalid vtcapture stride"
        );
        this.plane = plane;
        // A failed process may have partially started the driver; stop on drop.
        this.stage = Stage::Running;
        status(
            vt_call!(
                this,
                prysm_vt_client,
                this.api.process,
                this.client.as_mut_ptr()
            )?,
            "vtCapture_process",
        )?;
        Ok(this)
    }

    pub(super) fn capture(&mut self, output: &mut Vec<u8>) -> Result<Option<Header>> {
        let mut buffer = BufferInfo::default();
        let code = vt_call!(self, prysm_vt_buffer, self.api.buffer, &raw mut buffer)?;
        if code == 2 {
            // Allow the asynchronous producer to supply a frame before restarting it.
            let since = self.pending_since.get_or_insert_with(Instant::now);
            ensure!(
                since.elapsed() < Duration::from_secs(2),
                "vtCapture_currentCaptureBuffInfo status 2 persisted for two seconds"
            );
            return Ok(None);
        }
        if code == 17 && self.force_capture {
            force_capture()?;
        }
        status(code, "vtCapture_currentCaptureBuffInfo")?;
        ensure!(
            !buffer.y.is_null() && !buffer.uv.is_null(),
            "empty vtcapture planes"
        );
        let width = u32::try_from(self.plane.region[2])?;
        let height = u32::try_from(self.plane.region[3])?;
        let header = pixels::header(width, height, PixelFormat::Nv12)?;
        let stride = usize::try_from(self.plane.stride)?;
        // SAFETY: successful acquisition supplies driver-owned allocations;
        // no further driver call or release occurs before both copies finish.
        let (y, uv) = unsafe {
            (
                driver_plane(self, buffer.y, usize::try_from(buffer.y_len)?)?,
                driver_plane(self, buffer.uv, usize::try_from(buffer.uv_len)?)?,
            )
        };
        pixels::copy_plane(y, stride, width as usize, height as usize, output)?;
        pixels::copy_plane(uv, stride, width as usize, height as usize / 2, output)?;
        self.pending_since = None;
        Ok(Some(header))
    }
}

impl Drop for VtCapture {
    fn drop(&mut self) {
        for (needed, function) in [
            (self.stage >= Stage::Running, self.api.stop),
            (self.stage >= Stage::Prepared, self.api.postprocess),
            (self.stage >= Stage::Initialized, self.api.finalize),
        ] {
            if needed {
                match vt_call!(self, prysm_vt_client, function, self.client.as_mut_ptr()) {
                    Ok(0) => {}
                    result => tracing::warn!(?result, "vtcapture cleanup failed"),
                }
            }
        }
        if let Err(error) = vt_call!(self, prysm_vt_release, self.api.release) {
            tracing::warn!(error = %format!("{error:#}"), "vtcapture cleanup failed");
        }
    }
}

fn force_capture() -> Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/forcecapture")?;
    // SAFETY: the forcecapture kernel interface defines _IO('Z', 0), no pointer.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), 0x5a00 as libc::c_ulong) };
    if result < 0 {
        return Err(std::io::Error::last_os_error()).context("forcecapture ioctl");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::Video;
    use clap::Parser;
    use std::{
        io::Read,
        net::TcpListener,
        sync::atomic::{AtomicBool, Ordering},
    };

    struct Driver {
        code: i32,
        pending_polls: usize,
        y: [u8; 8],
        uv: [u8; 4],
        cleanup: Vec<u8>,
    }
    unsafe extern "C" fn fake_buffer(driver: *mut c_void, output: *mut BufferInfo) -> i32 {
        // SAFETY: test owns Driver and BufferInfo for the duration of this call.
        unsafe {
            let driver = &mut *driver.cast::<Driver>();
            if driver.pending_polls > 0 {
                driver.pending_polls -= 1;
                return 2;
            }
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
        unsafe {
            (*driver.cast::<Driver>()).cleanup.push(1);
        }
        0
    }
    unsafe extern "C" fn fake_postprocess(driver: *mut c_void, _: *mut c_char) -> i32 {
        // SAFETY: test-owned driver stays alive until after the capture drops.
        unsafe {
            (*driver.cast::<Driver>()).cleanup.push(2);
        }
        0
    }
    unsafe extern "C" fn fake_finalize(driver: *mut c_void, _: *mut c_char) -> i32 {
        // SAFETY: test-owned driver stays alive until after the capture drops.
        unsafe {
            (*driver.cast::<Driver>()).cleanup.push(3);
        }
        0
    }
    unsafe extern "C" fn fake_release(driver: *mut c_void) {
        // SAFETY: test owns the driver and checks the event list after drop.
        unsafe {
            (*driver.cast::<Driver>()).cleanup.push(4);
        }
    }
    unsafe extern "C" fn unused_create() -> *mut c_void {
        ptr::null_mut()
    }
    unsafe extern "C" fn unused_init(_: *mut c_void, _: *const c_char, _: *mut c_char) -> i32 {
        0
    }
    unsafe extern "C" fn unused_preprocess(
        _: *mut c_void,
        _: *mut c_char,
        _: *mut Properties,
    ) -> i32 {
        0
    }
    unsafe extern "C" fn unused_plane(_: *mut c_void, _: *mut c_char, _: *mut PlaneInfo) -> i32 {
        0
    }

    fn fake_capture(driver: &mut Driver) -> Video {
        let api = VtApi {
            create: unused_create,
            init: unused_init,
            preprocess: unused_preprocess,
            plane: unused_plane,
            process: fake_stop,
            buffer: fake_buffer,
            stop: fake_stop,
            postprocess: fake_postprocess,
            finalize: fake_finalize,
            release: fake_release,
            _library: libloading::os::unix::Library::this().into(),
        };
        Video::Vt(VtCapture {
            api,
            driver: NonNull::from(driver).cast(),
            client: [0; 128],
            stage: Stage::Running,
            plane: PlaneInfo {
                stride: 4,
                region: [0, 0, 2, 2],
                active: [0; 4],
            },
            force_capture: false,
            pending_since: None,
        })
    }

    #[test]
    fn reused_buffers_are_copied_and_errors_never_publish_stale_pixels() {
        let mut driver = Driver {
            code: 0,
            pending_polls: 0,
            y: [16, 235, 99, 99, 16, 235, 99, 99],
            uv: [128, 128, 99, 99],
            cleanup: Vec::new(),
        };
        let mut capture = fake_capture(&mut driver);
        let mut output = Vec::new();
        let header = capture.capture(&mut output).unwrap().unwrap();
        assert_eq!(header.format, PixelFormat::Nv12);
        assert_eq!(output, [16, 235, 16, 235, 128, 128]);
        driver.y[0] = 235;
        capture.capture(&mut output).unwrap();
        assert_eq!(output, [235, 235, 16, 235, 128, 128]);
        driver.code = 12;
        assert!(capture.capture(&mut output).is_err());
        assert_eq!(output, [] as [u8; 0]);
        drop(capture);
        assert_eq!(driver.cleanup, [1, 2, 3, 4]);
    }

    #[test]
    fn initial_status_two_keeps_driver_alive_until_frames_arrive() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cli = crate::cli::Cli::try_parse_from([
            "capture",
            &listener.local_addr().unwrap().to_string(),
            "--width",
            "2",
            "--height",
            "2",
        ])
        .unwrap();
        let mut driver = Driver {
            code: 0,
            pending_polls: 2,
            y: [235; 8],
            uv: [128; 4],
            cleanup: Vec::new(),
        };
        let mut capture = Some(fake_capture(&mut driver));
        let receiver = std::thread::spawn(move || crate::transport::tests::accept(&listener, 30));
        let stop = AtomicBool::new(false);
        let mut ui_frames = 0;
        crate::session::capture_loop(
            &cli,
            &stop,
            |_| {
                Ok(capture
                    .take()
                    .expect("status 2 must not restart the driver"))
            },
            Some(|_, _, bgra: &mut crate::pixels::Bgra8Pixels| {
                ui_frames += 1;
                bgra.bytes_mut().resize(16, 0);
                if ui_frames == 3 {
                    stop.store(true, Ordering::Relaxed);
                }
                Ok(())
            }),
        )
        .unwrap();
        let mut socket = receiver.join().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        for expected in [0, 0, 255] {
            let mut header = [0; prysm_webos_protocol::HEADER_LEN];
            socket.read_exact(&mut header).unwrap();
            let header = Header::decode(&header).unwrap();
            let mut pixels = vec![0; header.payload_len];
            socket.read_exact(&mut pixels).unwrap();
            assert!(
                pixels.iter().all(|value| value.abs_diff(expected) <= 1),
                "expected UI-only black while waiting, then white video; got {pixels:?}"
            );
        }
        assert_eq!(driver.pending_polls, 0);
        assert_eq!(driver.cleanup, [1, 2, 3, 4]);
    }

    #[test]
    fn status_two_wait_is_bounded_and_resets_after_success() {
        let mut driver = Driver {
            code: 2,
            pending_polls: 0,
            y: [16; 8],
            uv: [128; 4],
            cleanup: Vec::new(),
        };
        let mut capture = fake_capture(&mut driver);
        let mut output = vec![255; 6];
        assert!(capture.capture(&mut output).unwrap().is_none());
        assert_eq!(output, [] as [u8; 0]);
        if let Video::Vt(vt) = &mut capture {
            vt.pending_since = Some(Instant::now().checked_sub(Duration::from_secs(3)).unwrap());
        }
        assert!(
            capture
                .capture(&mut output)
                .unwrap_err()
                .to_string()
                .contains("status 2 persisted")
        );
        driver.code = 0;
        assert!(capture.capture(&mut output).unwrap().is_some());
        driver.code = 2;
        assert!(capture.capture(&mut output).unwrap().is_none());
        assert_eq!(output, [] as [u8; 0]);
        assert_eq!(driver.cleanup, [] as [u8; 0]);
        for code in [12, 17, 99] {
            driver.code = code;
            assert!(capture.capture(&mut output).is_err());
        }
    }
}
