use super::driver_plane;
use super::ffi::{
    BufferInfo, PlaneInfo, Properties, VtBuffer, VtClient, VtCreate, VtInit, VtPlane, VtPreprocess,
    VtRelease, prysm_vt_buffer, prysm_vt_client, prysm_vt_create, prysm_vt_init, prysm_vt_plane,
    prysm_vt_preprocess, prysm_vt_release,
};
use super::{library, status};
use crate::{config::CaptureOptions, pixels};
use anyhow::{Context, Result, ensure};
use libloading::Library;
use prysm_webos_protocol::{Header, PixelFormat};
use std::ffi::CStr;
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
    properties: Properties,
    prepare_at: Instant,
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
        let properties = Properties {
            dump: 2, // LG display capture tap.
            width: i16::try_from(options.width)?,
            height: i16::try_from(options.height)?,
            buffers: 3,
            fps: i32::from(options.fps),
            ..Properties::default()
        };
        let api = VtApi::load(&["libvtcapture.so.1", "libvtcapture.so"])?;
        super::permissions::check().map_err(super::FatalVideo)?;
        let mut driver = ptr::null_mut();
        let mut error = [0_u8; 512];
        // SAFETY: typed create symbol and valid out parameters; C++ contains throws.
        let code =
            unsafe { prysm_vt_create(api.create, &raw mut driver, error.as_mut_ptr().cast()) };
        if let Err(error) =
            vt_result(code, &error).and_then(|code| status(code, "vtCapture_create"))
        {
            // Failed construction can leave live threads and stale global pointers.
            std::mem::forget(api);
            return Err(super::FatalVideo(error).into());
        }
        let driver = NonNull::new(driver).context("vtcapture returned a null driver")?;
        let mut this = Self {
            api,
            driver,
            client: [0; 128],
            stage: Stage::Created,
            plane: PlaneInfo::default(),
            properties,
            prepare_at: Instant::now(),
            pending_since: None,
        };
        this.client[..2].fill(c_char::try_from(b'0')?);
        this.prepare()?;
        Ok(this)
    }

    fn prepare(&mut self) -> Result<bool> {
        self.prepare_at = Instant::now() + Duration::from_secs(1);
        if self.stage == Stage::Created {
            let code = vt_call!(
                self,
                prysm_vt_init,
                self.api.init,
                c"prysm-webos-capture".as_ptr(),
                self.client.as_mut_ptr()
            )?;
            if code == 1 {
                return self.retry_initialization();
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
            return self.retry_initialization();
        }
        status(code, "vtCapture_preprocess")?;
        self.pending_since = None;
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
        pixels::header(
            u32::try_from(plane.region[2])?,
            u32::try_from(plane.region[3])?,
            PixelFormat::Nv12,
        )?;
        ensure!(
            plane.stride >= i32::from(plane.region[2]) && plane.stride <= 32768,
            "invalid vtcapture stride"
        );
        self.plane = plane;
        // A failed process may have partially started the driver; stop on drop.
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
        tracing::info!(
            width = self.plane.region[2],
            height = self.plane.region[3],
            "video capture started"
        );
        Ok(true)
    }

    fn retry_initialization(&mut self) -> Result<bool> {
        // LG finalizes the session on status 1; preprocessing alone cannot reopen its device.
        self.stage = Stage::Created;
        if self.pending_since.is_none() {
            tracing::info!("video session finalized; retrying initialization");
        }
        let since = self.pending_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= Duration::from_secs(10) {
            return Err(super::FatalVideo(anyhow::anyhow!(
                "video initialization remained not ready for ten seconds; restarting worker"
            ))
            .into());
        }
        Ok(false)
    }

    pub(super) fn capture(
        &mut self,
        output: &mut Vec<u8>,
        events: &mut crate::events::Events,
    ) -> Result<Option<Header>> {
        if self.stage <= Stage::Initialized {
            if Instant::now() < self.prepare_at {
                return Ok(None);
            }
            events.send(&crate::events::Event::VideoResetting)?;
            let result = self.prepare();
            events.send(&crate::events::Event::VideoReset)?;
            if !result? {
                return Ok(None);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{events::Events, native::Video};
    use std::{
        io::Read,
        net::TcpListener,
        sync::atomic::{AtomicBool, Ordering},
    };

    struct Driver {
        code: i32,
        initialized: bool,
        initializations: usize,
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
    unsafe extern "C" fn fake_init(driver: *mut c_void, _: *const c_char, _: *mut c_char) -> i32 {
        // SAFETY: the test owns Driver throughout initialization and capture.
        let driver = unsafe { &mut *driver.cast::<Driver>() };
        driver.initializations += 1;
        driver.initialized = driver.code == 0;
        driver.code
    }

    fn fake_capture(driver: &mut Driver) -> Video {
        let api = VtApi {
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
            properties: Properties::default(),
            prepare_at: Instant::now(),
            pending_since: None,
        })
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
        let code = driver.code;
        // LG preprocessing failure finalizes its session; preprocessing alone cannot reopen it.
        if code != 0 {
            driver.initialized = false;
        }
        code
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

    #[test]
    fn preprocessing_failure_reinitializes_the_finalized_session_without_recreating_driver() {
        let mut events = Events::disabled();
        let mut driver = Driver {
            code: 1,
            initialized: true,
            initializations: 0,
            pending_polls: 0,
            y: [16; 8],
            uv: [128; 4],
            cleanup: Vec::new(),
        };
        let mut capture = fake_capture(&mut driver);
        if let Video::Vt(vt) = &mut capture {
            vt.stage = Stage::Initialized;
        }
        let mut output = vec![255; 6];
        assert!(capture.capture(&mut output, &mut events).unwrap().is_none());
        assert_eq!(output, [] as [u8; 0]);
        assert_eq!(driver.cleanup, [] as [u8; 0]);
        driver.code = 99;
        assert!(capture.capture(&mut output, &mut events).unwrap().is_none());
        driver.code = 1;
        if let Video::Vt(vt) = &mut capture {
            vt.prepare_at = Instant::now();
        }
        assert!(capture.capture(&mut output, &mut events).unwrap().is_none());
        assert_eq!(driver.initializations, 1);
        driver.code = 0;
        if let Video::Vt(vt) = &mut capture {
            vt.prepare_at = Instant::now();
        }
        assert!(capture.capture(&mut output, &mut events).unwrap().is_some());
        assert_eq!(driver.initializations, 2);
        assert_eq!(output, [16, 16, 16, 16, 128, 128]);
        drop(capture);
        assert_eq!(driver.cleanup, [1, 2, 3, 4]);
    }

    #[test]
    fn persistent_not_ready_reports_failure_before_native_teardown() {
        let mut events = Events::disabled();
        let config = crate::config::Config::new("127.0.0.1:19400".parse().unwrap());
        let mut driver = Driver {
            code: 1,
            initialized: true,
            initializations: 0,
            pending_polls: 0,
            y: [16; 8],
            uv: [128; 4],
            cleanup: Vec::new(),
        };
        let mut video = fake_capture(&mut driver);
        if let Video::Vt(vt) = &mut video {
            vt.stage = Stage::Initialized;
            vt.prepare_at = Instant::now();
            vt.pending_since = Instant::now().checked_sub(Duration::from_secs(11));
        }
        let mut video = Some(video);
        let mut capture = crate::capture::Capture::new(&config);
        let mut ui = Some(|width, height, bgra: &mut crate::pixels::Bgra8Pixels| {
            bgra.bytes_mut().resize((width * height * 4) as usize, 0);
            Ok(())
        });
        let error = capture
            .capture(&mut |_| Ok(video.take().unwrap()), &mut ui, &mut events)
            .err()
            .expect("a permanently unready client must request worker recovery");
        assert!(error.is::<crate::native::FatalVideo>());
        assert_eq!(
            driver.cleanup,
            [] as [u8; 0],
            "main must report the failure before potentially blocking native teardown"
        );
        drop(capture);
        assert_eq!(driver.cleanup, [4]);
    }

    #[test]
    fn acquisition_error_arms_watchdog_before_native_cleanup() {
        use std::{
            cell::RefCell,
            os::{fd::OwnedFd, unix::net::UnixStream},
        };
        thread_local! {
            static OBSERVED: RefCell<Option<(UnixStream, Vec<u8>)>> = const { RefCell::new(None) };
        }
        unsafe extern "C" fn observe_stop(driver: *mut c_void, client: *mut c_char) -> i32 {
            OBSERVED.with_borrow_mut(|observed| {
                if let Some((reader, bytes)) = observed {
                    let mut buffer = [0; 128];
                    if let Ok(count) = reader.read(&mut buffer) {
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                }
            });
            // SAFETY: the caller retains the same test-owned driver through cleanup.
            unsafe { fake_stop(driver, client) }
        }
        let mut driver = Driver {
            code: 0,
            initialized: true,
            initializations: 0,
            pending_polls: 0,
            y: [16; 8],
            uv: [128; 4],
            cleanup: Vec::new(),
        };
        let mut video = fake_capture(&mut driver);
        if let Video::Vt(vt) = &mut video {
            vt.api.stop = observe_stop;
        }
        let (writer, mut reader) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let writer: OwnedFd = writer.into();
        let mut events = Events::from_file(writer.into());
        let config = crate::config::Config {
            source: prysm_webos_protocol::Source::Video,
            ..crate::config::Config::new("127.0.0.1:19400".parse().unwrap())
        };
        let mut capture = crate::capture::Capture::new(&config);
        let mut video = Some(video);
        let mut ui = None::<fn(u32, u32, &mut crate::pixels::Bgra8Pixels) -> Result<()>>;
        assert!(
            capture
                .capture(&mut |_| Ok(video.take().unwrap()), &mut ui, &mut events)
                .unwrap()
                .is_some()
        );
        let startup = b"{\"type\":\"video_resetting\"}\n{\"type\":\"video_reset\"}\n";
        let mut bytes = vec![0; startup.len()];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, startup);
        OBSERVED.with_borrow_mut(|observed| *observed = Some((reader, Vec::new())));
        driver.code = 99;
        assert!(
            capture
                .capture(
                    &mut |_| panic!("reopened before backoff"),
                    &mut ui,
                    &mut events
                )
                .unwrap()
                .is_none()
        );
        let (mut reader, bytes) = OBSERVED.with_borrow_mut(Option::take).unwrap();
        assert_eq!(bytes, b"{\"type\":\"video_resetting\"}\n");
        assert_eq!(driver.cleanup, [1, 2, 3, 4]);
        let mut completed = [0; 23];
        reader.read_exact(&mut completed).unwrap();
        assert_eq!(&completed, b"{\"type\":\"video_reset\"}\n");
    }

    #[test]
    fn successful_preparation_clears_the_old_wait_before_buffer_polling() {
        use std::os::{fd::OwnedFd, unix::net::UnixStream};
        let (writer, mut reader) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let writer: OwnedFd = writer.into();
        let mut events = Events::from_file(writer.into());
        let mut driver = Driver {
            code: 0,
            initialized: true,
            initializations: 0,
            pending_polls: 1,
            y: [16; 8],
            uv: [128; 4],
            cleanup: Vec::new(),
        };
        let mut video = fake_capture(&mut driver);
        if let Video::Vt(vt) = &mut video {
            vt.stage = Stage::Initialized;
            vt.prepare_at = Instant::now();
            vt.pending_since = Instant::now().checked_sub(Duration::from_secs(11));
        }
        let mut output = Vec::new();
        assert!(video.capture(&mut output, &mut events).unwrap().is_none());
        assert!(video.capture(&mut output, &mut events).unwrap().is_some());
        let expected = b"{\"type\":\"video_resetting\"}\n{\"type\":\"video_reset\"}\n";
        let mut bytes = vec![0; expected.len()];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, expected);
        assert_eq!(
            reader.read(&mut bytes).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(video);
        assert_eq!(driver.cleanup, [1, 2, 3, 4]);
    }

    #[test]
    fn reused_buffers_are_copied_and_errors_never_publish_stale_pixels() {
        let mut events = Events::disabled();
        let mut driver = Driver {
            code: 0,
            initialized: true,
            initializations: 0,
            pending_polls: 0,
            y: [16, 235, 99, 99, 16, 235, 99, 99],
            uv: [128, 128, 99, 99],
            cleanup: Vec::new(),
        };
        let mut capture = fake_capture(&mut driver);
        let mut output = Vec::new();
        let header = capture.capture(&mut output, &mut events).unwrap().unwrap();
        assert_eq!(header.format, PixelFormat::Nv12);
        assert_eq!(output, [16, 235, 16, 235, 128, 128]);
        driver.y[0] = 235;
        capture.capture(&mut output, &mut events).unwrap();
        assert_eq!(output, [235, 235, 16, 235, 128, 128]);
        driver.code = 12;
        assert!(capture.capture(&mut output, &mut events).is_err());
        assert_eq!(output, [] as [u8; 0]);
        drop(capture);
        assert_eq!(driver.cleanup, [1, 2, 3, 4]);
    }

    #[test]
    fn initial_status_two_keeps_driver_alive_until_frames_arrive() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config =
            crate::config::Config::new(listener.local_addr().unwrap().to_string().parse().unwrap());
        let mut driver = Driver {
            code: 0,
            initialized: true,
            initializations: 0,
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
            &config,
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
            let mut rgb = Vec::new();
            crate::pixels::to_rgb(header, &pixels, &mut rgb).unwrap();
            assert!(
                rgb.iter().all(|value| value.abs_diff(expected) <= 1),
                "expected UI-only black while waiting, then white video; got {rgb:?}"
            );
        }
        assert_eq!(driver.pending_polls, 0);
        assert_eq!(driver.cleanup, [1, 2, 3, 4]);
    }

    #[test]
    fn status_two_wait_is_bounded_and_resets_after_success() {
        let mut events = Events::disabled();
        let mut driver = Driver {
            code: 2,
            initialized: true,
            initializations: 0,
            pending_polls: 0,
            y: [16; 8],
            uv: [128; 4],
            cleanup: Vec::new(),
        };
        let mut capture = fake_capture(&mut driver);
        let mut output = vec![255; 6];
        assert!(capture.capture(&mut output, &mut events).unwrap().is_none());
        assert_eq!(output, [] as [u8; 0]);
        if let Video::Vt(vt) = &mut capture {
            vt.pending_since = Some(Instant::now().checked_sub(Duration::from_secs(3)).unwrap());
        }
        assert!(
            capture
                .capture(&mut output, &mut events)
                .unwrap_err()
                .to_string()
                .contains("status 2 persisted")
        );
        driver.code = 0;
        assert!(capture.capture(&mut output, &mut events).unwrap().is_some());
        driver.code = 2;
        assert!(capture.capture(&mut output, &mut events).unwrap().is_none());
        assert_eq!(output, [] as [u8; 0]);
        assert_eq!(driver.cleanup, [] as [u8; 0]);
        for code in [12, 17, 99] {
            driver.code = code;
            assert!(capture.capture(&mut output, &mut events).is_err());
        }
    }
}
