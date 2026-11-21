//! Research probe: what does LG's `vtCapture_capabilityInfo` write?
//!
//! Not part of Prysm. Bead `prysm-884`; method and decode plan in
//! `research/webos-capture/2026-11-21-capability-info/README.md`.
//!
//! The only public declaration of `_LibVtCaptureCapabilityInfo`
//! (tv-native-apis, copied into `native/vtCaptureApi_c.h`) is a placeholder of
//! eleven pointers. This probe hands the library a buffer far larger than that,
//! in which every word is the address of its own sentinel-filled scratch
//! block, surrounded by guard words. Afterwards:
//!
//! - a changed word is a value the library wrote there;
//! - a changed scratch block means the library used that word as a pointer
//!   and wrote through it (the word itself is then unchanged);
//! - the last changed byte gives the size the library writes;
//! - a changed guard word is a write outside the probe area.
//!
//! Sequence, on one thread with SIGINT and SIGTERM blocked (as Prysm's capture
//! thread): take `/tmp/prysm-capture.lock` (the worker's lock; refuses while a
//! worker holds it), check the Luna role, load `libvtcapture`,
//! `vtCapture_create`, `vtCapture_init`, `vtCapture_capabilityInfo`; with
//! `--preprocess`, also `vtCapture_preprocess` (640x360, 30 fps, video only,
//! 3 buffers) and the call again. `vtCapture_process` is never called, so
//! capture never starts. Release follows the stage reached, as
//! `VtCapture::cleanup`: postprocess (if preprocessed), finalize (if
//! initialized), release. Every native call has a 5 s watchdog (4 s for
//! release), as the worker's `Limits::TV`; a call that overruns ends the
//! process, which is how the supervisor recovers a hung worker too. SIGINT or
//! SIGTERM between calls skips to the release. It writes no files except the
//! lock file, which it never deletes.
//!
//! Needs root and a Luna role for its own executable path (see the research
//! note's `run-tv.sh`). Elsewhere it refuses to start (exit 2).
//!
//! Exit: 0 dump printed, 1 setup or native failure, 2 not a TV / bad
//! arguments, 3 watchdog, 4 dump printed but a call returned non-zero or
//! cleanup failed.
use prysm_capture_webos_sys as _; // links the C++ exception boundary used below
use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs::File;
use std::io::Write as _;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Body of the buffer handed to the library, in machine words (256 bytes on
/// the TV; the placeholder declares 44).
const BODY_WORDS: usize = 64;
/// Guard words before and after the body.
const GUARD_WORDS: usize = 16;
/// Bytes of scratch behind each body word.
const SCRATCH: usize = 64;
const SCRATCH_FILL: u8 = 0x5A;
const LOCK: &str = "/tmp/prysm-capture.lock";
const CALL_LIMIT: Duration = Duration::from_secs(5);
const RELEASE_LIMIT: Duration = Duration::from_secs(4);

fn guard(index: usize) -> u32 {
    0xA5A5_0000 | u32::try_from(index).unwrap_or(0xFFFF)
}

/// What the library receives: `body` is passed as the struct.
#[repr(C)]
struct Area {
    before: [u32; GUARD_WORDS],
    body: [usize; BODY_WORDS],
    after: [u32; GUARD_WORDS],
}

/// The buffer and the scratch blocks its words point to.
struct Probe {
    area: Box<Area>,
    scratch: Box<[[u8; SCRATCH]; BODY_WORDS]>,
    initial: Vec<u8>,
}

impl Probe {
    fn new() -> Self {
        let scratch = Box::new([[SCRATCH_FILL; SCRATCH]; BODY_WORDS]);
        let mut area = Box::new(Area {
            before: [0; GUARD_WORDS],
            body: [0; BODY_WORDS],
            after: [0; GUARD_WORDS],
        });
        for (index, word) in area.before.iter_mut().enumerate() {
            *word = guard(index);
        }
        for (index, word) in area.after.iter_mut().enumerate() {
            *word = guard(GUARD_WORDS + index);
        }
        for (word, block) in area.body.iter_mut().zip(scratch.iter()) {
            *word = block.as_ptr() as usize;
        }
        let initial = body_bytes(&area);
        Self {
            area,
            scratch,
            initial,
        }
    }

    fn body_ptr(&mut self) -> *mut c_void {
        self.area.body.as_mut_ptr().cast()
    }

    fn report(&self) -> Report {
        let body = body_bytes(&self.area);
        let changed: Vec<usize> = (0..body.len())
            .filter(|&i| body[i] != self.initial[i])
            .collect();
        let pointers = self
            .scratch
            .iter()
            .enumerate()
            .filter(|(_, block)| block.iter().any(|&b| b != SCRATCH_FILL))
            .map(|(index, block)| (index, block.to_vec()))
            .collect();
        let guards = |words: &[u32], offset: usize| {
            words
                .iter()
                .enumerate()
                .filter(|&(i, &w)| w != guard(offset + i))
                .map(|(i, &w)| (i, w))
                .collect::<Vec<_>>()
        };
        Report {
            // Whole 32-bit words: a written byte equal to the pointer byte it
            // replaced (often a zero high byte) is invisible.
            extent: changed.last().map_or(0, |last| (last / 4 + 1) * 4),
            changed,
            body,
            initial: self.initial.clone(),
            pointers,
            before: guards(&self.area.before, 0),
            after: guards(&self.area.after, GUARD_WORDS),
        }
    }
}

fn body_bytes(area: &Area) -> Vec<u8> {
    area.body.iter().flat_map(|w| w.to_ne_bytes()).collect()
}

struct Report {
    body: Vec<u8>,
    initial: Vec<u8>,
    /// Changed body byte offsets.
    changed: Vec<usize>,
    /// The end of the last changed 32-bit word: the size written, at least.
    extent: usize,
    /// Body word index whose scratch block changed, and that block.
    pointers: Vec<(usize, Vec<u8>)>,
    /// Changed guard words (index, value).
    before: Vec<(usize, u32)>,
    after: Vec<(usize, u32)>,
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("4 bytes"))
}

fn half(value: u32, high: bool) -> u16 {
    (if high { value >> 16 } else { value & 0xFFFF }) as u16
}

impl Report {
    fn print(&self, label: &str, out: &mut impl std::io::Write) -> std::io::Result<()> {
        let pointer_bytes = size_of::<usize>();
        writeln!(out, "# {label}: written extent {} bytes", self.extent)?;
        writeln!(out, "# {label}: changed body bytes {}", self.changed.len())?;
        writeln!(
            out,
            "# {label}: guard before {}, guard after {}",
            if self.before.is_empty() {
                "intact"
            } else {
                "CHANGED"
            },
            if self.after.is_empty() {
                "intact"
            } else {
                "CHANGED (write beyond 256 bytes)"
            }
        )?;
        for (index, value) in self.before.iter().chain(&self.after) {
            writeln!(out, "# {label}: guard word {index} = {value:#010x}")?;
        }
        writeln!(out, "## hex (offset: bytes, * = changed)")?;
        let rows = self.extent.div_ceil(16).max(4) * 16;
        for row in (0..rows.min(self.body.len())).step_by(16) {
            let mut line = format!("{row:04x}:");
            for i in row..row + 16 {
                let mark = if self.body[i] == self.initial[i] {
                    ' '
                } else {
                    '*'
                };
                line.push_str(&format!(" {:02x}{mark}", self.body[i]));
            }
            writeln!(out, "{line}")?;
        }
        writeln!(
            out,
            "## words (offset hex u32 i32 lo16/hi16 as u16 and i16, state)"
        )?;
        for offset in (0..rows.min(self.body.len())).step_by(4) {
            let value = word(&self.body, offset);
            let state = if value != word(&self.initial, offset) {
                "written".to_owned()
            } else if pointer_bytes == 4
                && let Some((_, block)) = self.pointers.iter().find(|(i, _)| *i * 4 == offset)
            {
                format!("untouched; wrote through it: {}", hex(block))
            } else {
                "untouched".to_owned()
            };
            writeln!(
                out,
                "{offset:#06x} {value:#010x} {value:>10} {:>11} {:>5}/{:<5} {:>6}/{:<6} {state}",
                value.cast_signed(),
                half(value, false),
                half(value, true),
                half(value, false).cast_signed(),
                half(value, true).cast_signed(),
            )?;
        }
        if pointer_bytes != 4 {
            for (index, block) in &self.pointers {
                writeln!(out, "# {label}: wrote through word {index}: {}", hex(block))?;
            }
        }
        self.print_candidate(label, out)
    }

    /// Layout A (research note): the kernel's `v4l2_ext_capture_capability_info`.
    fn print_candidate(&self, label: &str, out: &mut impl std::io::Write) -> std::io::Result<()> {
        let w = |offset| word(&self.body, offset);
        let rect = w(24);
        let rect2 = w(28);
        writeln!(
            out,
            "# {label}: if layout A: flags={:#x} scale_up={}x{} scale_down={}x{} buffers={} \
             max_res=({},{} {}x{}) planes={} pixel_format={}",
            w(0),
            w(4),
            w(8),
            w(12),
            w(16),
            w(20),
            half(rect, false),
            half(rect, true),
            half(rect2, false),
            half(rect2, true),
            w(32),
            w(36),
        )
    }
}

fn hex(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .rposition(|&b| b != SCRATCH_FILL)
        .map_or(0, |i| i + 1);
    bytes[..end].iter().map(|b| format!("{b:02x}")).collect()
}

// --- LG's API, through the crate's C++ exception boundary (native/vtcapture.cpp)

type Create = unsafe extern "C" fn() -> *mut c_void;
type Init = unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_char) -> c_int;
/// `vtCapture_preprocess`, `vtCapture_capabilityInfo` (and `planeInfo`):
/// driver, client, out struct.
type WithStruct = unsafe extern "C" fn(*mut c_void, *mut c_char, *mut c_void) -> c_int;
type Client = unsafe extern "C" fn(*mut c_void, *mut c_char) -> c_int;
type Release = unsafe extern "C" fn(*mut c_void);

// Declared in `native/vtcapture.cpp` with typed struct pointers; every data
// pointer has the same representation in the ARM EABI and AAPCS64, so the
// untyped `*mut c_void` here calls them identically.
unsafe extern "C" {
    fn prysm_vt_create(function: Create, driver: *mut *mut c_void, error: *mut c_char) -> c_int;
    fn prysm_vt_init(
        function: Init,
        driver: *mut c_void,
        caller: *const c_char,
        client: *mut c_char,
        error: *mut c_char,
    ) -> c_int;
    fn prysm_vt_preprocess(
        function: WithStruct,
        driver: *mut c_void,
        client: *mut c_char,
        properties: *mut c_void,
        error: *mut c_char,
    ) -> c_int;
    /// Used for `vtCapture_capabilityInfo`: same shape as `planeInfo`.
    fn prysm_vt_plane(
        function: WithStruct,
        driver: *mut c_void,
        client: *mut c_char,
        output: *mut c_void,
        error: *mut c_char,
    ) -> c_int;
    fn prysm_vt_client(
        function: Client,
        driver: *mut c_void,
        client: *mut c_char,
        error: *mut c_char,
    ) -> c_int;
    fn prysm_vt_release(function: Release, driver: *mut c_void, error: *mut c_char) -> c_int;
}

/// `_LibVtCaptureProperties` (as `ffi::Properties`).
#[repr(C)]
struct Properties {
    dump: i32,
    x: i16,
    y: i16,
    width: i16,
    height: i16,
    buffers: i32,
    fps: i32,
}

struct Api {
    create: Create,
    init: Init,
    preprocess: WithStruct,
    capability: WithStruct,
    postprocess: Client,
    finalize: Client,
    release: Release,
}

fn load() -> Result<Api, String> {
    let mut last = String::new();
    for name in ["libvtcapture.so.1", "libvtcapture.so"] {
        // SAFETY: LG's system library, loaded on the thread that also calls and
        // releases it, as Prysm does. Leaked: never unloaded (see vtcapture.rs).
        match unsafe { libloading::Library::new(name) } {
            Ok(library) => {
                let library: &'static libloading::Library = Box::leak(Box::new(library));
                let symbol = |symbol: &str| -> Result<*mut c_void, String> {
                    // SAFETY: only the address is taken; it is cast to the
                    // declared signature below and the library is never unloaded.
                    unsafe { library.get::<*mut c_void>(symbol.as_bytes()) }
                        .map(|s| *s)
                        .map_err(|e| format!("{symbol}: {e}"))
                };
                // SAFETY: signatures as in native/vtCaptureApi_c.h (tv-native-apis).
                return unsafe {
                    Ok(Api {
                        create: std::mem::transmute::<*mut c_void, Create>(symbol(
                            "vtCapture_create",
                        )?),
                        init: std::mem::transmute::<*mut c_void, Init>(symbol("vtCapture_init")?),
                        preprocess: std::mem::transmute::<*mut c_void, WithStruct>(symbol(
                            "vtCapture_preprocess",
                        )?),
                        capability: std::mem::transmute::<*mut c_void, WithStruct>(symbol(
                            "vtCapture_capabilityInfo",
                        )?),
                        postprocess: std::mem::transmute::<*mut c_void, Client>(symbol(
                            "vtCapture_postprocess",
                        )?),
                        finalize: std::mem::transmute::<*mut c_void, Client>(symbol(
                            "vtCapture_finalize",
                        )?),
                        release: std::mem::transmute::<*mut c_void, Release>(symbol(
                            "vtCapture_release",
                        )?),
                    })
                };
            }
            Err(error) => last = format!("{name}: {error}"),
        }
    }
    Err(last)
}

// --- watchdog and signals

static STOP: AtomicBool = AtomicBool::new(false);
/// Milliseconds since `START` by which the current call must return; 0: none.
static DEADLINE: AtomicU64 = AtomicU64::new(0);
static STAGE: Mutex<&str> = Mutex::new("start");
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn elapsed_ms() -> u64 {
    u64::try_from(START.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Run one native call under the watchdog, logging it first (a hang never
/// logs after).
fn guarded<T>(stage: &'static str, limit: Duration, call: impl FnOnce() -> T) -> T {
    *STAGE.lock().unwrap_or_else(PoisonError::into_inner) = stage;
    println!("# call {stage} at {} ms", elapsed_ms());
    let _ = std::io::stdout().flush();
    let limit = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX);
    DEADLINE.store(elapsed_ms().saturating_add(limit), Ordering::SeqCst);
    let result = call();
    DEADLINE.store(0, Ordering::SeqCst);
    result
}

/// Block SIGINT and SIGTERM in this thread and every thread it starts, then
/// take them on a thread of their own; start the watchdog.
fn supervise() -> Result<(), String> {
    // SAFETY: plain libc signal-set calls on locals.
    let set = unsafe {
        let mut set = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&raw mut set);
        libc::sigaddset(&raw mut set, libc::SIGINT);
        libc::sigaddset(&raw mut set, libc::SIGTERM);
        if libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut()) != 0 {
            return Err("cannot block SIGINT/SIGTERM".into());
        }
        set
    };
    std::thread::spawn(move || {
        loop {
            let mut signal = 0;
            // SAFETY: `set` is an initialized signal set owned by this thread.
            if unsafe { libc::sigwait(&raw const set, &raw mut signal) } == 0 {
                eprintln!("# signal {signal}: releasing after the current call");
                STOP.store(true, Ordering::SeqCst);
            }
        }
    });
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let deadline = DEADLINE.load(Ordering::SeqCst);
            if deadline != 0 && elapsed_ms() > deadline {
                let stage = *STAGE.lock().unwrap_or_else(PoisonError::into_inner);
                eprintln!("# watchdog: {stage} did not return; exiting without release");
                let _ = std::io::stdout().flush();
                // SAFETY: immediate exit, as a supervisor kill; runs no destructors
                // that could touch the vendor state the hung call holds.
                unsafe { libc::_exit(3) };
            }
        }
    });
    Ok(())
}

// --- the session

fn status(code: c_int, error: &[u8; 512]) -> String {
    if code == -1000 {
        let message = CStr::from_bytes_until_nul(error)
            .map_or_else(|_| "unterminated".into(), |m| m.to_string_lossy());
        format!("C++ exception: {message}")
    } else {
        format!("status {code}")
    }
}

/// The worker's lock: same path, flags and mode (`backend::capture_lock`).
fn lock() -> Result<File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(LOCK)
        .map_err(|e| format!("cannot open {LOCK}: {e}"))?;
    file.try_lock()
        .map_err(|e| format!("a Prysm capturer holds {LOCK} ({e}); stop Prysm first"))?;
    Ok(file)
}

fn check_luna() -> Result<(), String> {
    let pid = std::process::id();
    let names = [
        format!("com.webos.rm.client.prysm.{pid}"),
        format!("com.webos.service.capture.client.prysm.{pid}"),
    ];
    prysm_webos_luna_sys::check_registration(&names)
        .map_err(|e| format!("Luna denies this executable's capture names: {e}"))
}

fn capability(api: &Api, driver: *mut c_void, client: &mut [c_char; 128], label: &str) -> bool {
    let mut probe = Probe::new();
    let mut error = [0_u8; 512];
    let output = probe.body_ptr();
    let code = guarded("vtCapture_capabilityInfo", CALL_LIMIT, || {
        // SAFETY: live driver and client; `output` points at 256 writable bytes
        // (plus guards) whose words point at writable scratch; the boundary
        // catches C++ exceptions.
        unsafe {
            prysm_vt_plane(
                api.capability,
                driver,
                client.as_mut_ptr(),
                output,
                error.as_mut_ptr().cast(),
            )
        }
    });
    println!(
        "# {label}: vtCapture_capabilityInfo {}",
        status(code, &error)
    );
    let _ = probe.report().print(label, &mut std::io::stdout());
    code == 0
}

fn session(preprocess: bool) -> Result<bool, String> {
    let _lock = lock()?;
    println!("# lock {LOCK} held");
    check_luna()?;
    println!("# Luna role allows the capture names");
    let api = load()?;
    println!("# libvtcapture loaded");
    let mut error = [0_u8; 512];
    let mut driver = std::ptr::null_mut();
    let code = guarded("vtCapture_create", CALL_LIMIT, || {
        // SAFETY: typed create symbol and a live out parameter; C++ throws are caught.
        unsafe { prysm_vt_create(api.create, &raw mut driver, error.as_mut_ptr().cast()) }
    });
    if code != 0 || driver.is_null() {
        // Prysm restarts the process after a failed create; so does this probe.
        return Err(format!("vtCapture_create failed: {}", status(code, &error)));
    }
    let mut client = [0 as c_char; 128];
    client[..2].fill(c_char::from_ne_bytes(*b"0"));
    let mut clean = true;
    let mut initialized = false;
    let mut prepared = false;
    if !STOP.load(Ordering::SeqCst) {
        let code = guarded("vtCapture_init", CALL_LIMIT, || {
            // SAFETY: live driver, NUL-terminated caller, 128-byte client buffer.
            unsafe {
                prysm_vt_init(
                    api.init,
                    driver,
                    c"prysm-capture-webos".as_ptr(),
                    client.as_mut_ptr(),
                    error.as_mut_ptr().cast(),
                )
            }
        });
        // SAFETY: the library NUL-terminates the client id it writes; the
        // buffer was zeroed and is 128 bytes.
        let id = unsafe { CStr::from_ptr(client.as_ptr()) }.to_string_lossy();
        println!("# vtCapture_init {} client {id:?}", status(code, &error));
        initialized = code == 0;
        clean &= initialized;
    }
    if initialized && !STOP.load(Ordering::SeqCst) {
        clean &= capability(&api, driver, &mut client, "after init");
    }
    if initialized && preprocess && !STOP.load(Ordering::SeqCst) {
        let mut properties = Properties {
            dump: 2,
            x: 0,
            y: 0,
            width: 640,
            height: 360,
            buffers: 3,
            fps: 30,
        };
        let code = guarded("vtCapture_preprocess", CALL_LIMIT, || {
            // SAFETY: live driver and client; `properties` matches the header.
            unsafe {
                prysm_vt_preprocess(
                    api.preprocess,
                    driver,
                    client.as_mut_ptr(),
                    (&raw mut properties).cast(),
                    error.as_mut_ptr().cast(),
                )
            }
        });
        println!(
            "# vtCapture_preprocess (640x360, 30 fps, dump 2, 3 buffers) {}",
            status(code, &error)
        );
        // Status 1: LG finalized the session itself (see VtCapture::prepare).
        prepared = code == 0;
        if code == 1 {
            initialized = false;
        }
        clean &= prepared;
        if prepared && !STOP.load(Ordering::SeqCst) {
            clean &= capability(&api, driver, &mut client, "after preprocess");
        }
    }
    // Release by stage reached, as VtCapture::cleanup (no stop: never processed).
    for (needed, function, stage) in [
        (prepared, api.postprocess, "vtCapture_postprocess"),
        (initialized, api.finalize, "vtCapture_finalize"),
    ] {
        if needed {
            let code = guarded(stage, CALL_LIMIT, || {
                // SAFETY: live driver and client at the stage that needs this step.
                unsafe {
                    prysm_vt_client(
                        function,
                        driver,
                        client.as_mut_ptr(),
                        error.as_mut_ptr().cast(),
                    )
                }
            });
            println!("# {stage} {}", status(code, &error));
            clean &= code == 0;
        }
    }
    let code = guarded("vtCapture_release", RELEASE_LIMIT, || {
        // SAFETY: the driver is released exactly once, after every other call.
        unsafe { prysm_vt_release(api.release, driver, error.as_mut_ptr().cast()) }
    });
    println!("# vtCapture_release {}", status(code, &error));
    Ok(clean && code == 0)
}

fn main() -> ExitCode {
    let mut preprocess = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--preprocess" => preprocess = true,
            _ => {
                eprintln!("usage: vt_capability_probe [--preprocess]");
                return ExitCode::from(2);
            }
        }
    }
    if !cfg!(all(target_os = "linux", target_arch = "arm")) {
        eprintln!("runs only on an LG webOS TV (linux, armv7)");
        return ExitCode::from(2);
    }
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("needs root");
        return ExitCode::from(2);
    }
    let _ = elapsed_ms();
    if let Err(error) = supervise() {
        eprintln!("{error}");
        return ExitCode::from(1);
    }
    println!(
        "# vt_capability_probe: body {} bytes, guards {} bytes each side, pointer {} bytes",
        BODY_WORDS * size_of::<usize>(),
        GUARD_WORDS * 4,
        size_of::<usize>()
    );
    match session(preprocess) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(4),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes layout A's values, and an i16 through the pointer in word 10.
    unsafe extern "C" fn fake(_: *mut c_void, _: *mut c_char, output: *mut c_void) -> c_int {
        let words = output.cast::<usize>();
        // SAFETY: the test passes a probe body of BODY_WORDS words.
        unsafe {
            // Last word 4 (YUV422 semi-planar), not 1: on a 64-bit host a
            // pointer's high bytes are often 0x01 00 00 00 and would hide it.
            let values: [u32; 10] = [0x18, 3840, 2160, 360, 240, 5, 0, 3840 | (2160 << 16), 2, 4];
            let bytes = output.cast::<u8>();
            for (i, v) in values.iter().enumerate() {
                std::ptr::copy_nonoverlapping(v.to_le_bytes().as_ptr(), bytes.add(i * 4), 4);
            }
            let pointer = *words.add(10) as *mut i16;
            pointer.write_unaligned(1920);
        }
        0
    }

    #[test]
    fn the_report_separates_values_pointers_and_extent() {
        let mut probe = Probe::new();
        let mut error = [0_u8; 512];
        let mut client = [0 as c_char; 128];
        let output = probe.body_ptr();
        // SAFETY: the fake writes inside the probe body and scratch only.
        let code = unsafe {
            prysm_vt_plane(
                fake,
                std::ptr::null_mut(),
                client.as_mut_ptr(),
                output,
                error.as_mut_ptr().cast(),
            )
        };
        assert_eq!(code, 0);
        let report = probe.report();
        assert_eq!(report.extent, 40);
        assert_eq!(report.pointers.len(), 1);
        assert_eq!(report.pointers[0].0, 10);
        assert_eq!(&report.pointers[0].1[..2], &1920_i16.to_ne_bytes());
        assert!(report.before.is_empty() && report.after.is_empty());
        let mut text = Vec::new();
        report.print("test", &mut text).unwrap();
        let text = String::from_utf8(text).unwrap();
        assert!(text.contains("written extent 40 bytes"), "{text}");
        assert!(
            text.contains("scale_up=3840x2160 scale_down=360x240 buffers=5 max_res=(0,0 3840x2160) planes=2 pixel_format=4"),
            "{text}"
        );
    }

    #[test]
    fn an_untouched_probe_reports_nothing_written() {
        let report = Probe::new().report();
        assert_eq!(report.extent, 0);
        assert!(report.changed.is_empty() && report.pointers.is_empty());
        assert!(report.before.is_empty() && report.after.is_empty());
    }
}
