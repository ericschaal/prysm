//! LS2/GLib boundary. No raw pointer or native type leaves this module.
//!
//! A private `GLib` context dispatches callbacks on the run thread. Callback state
//! has a shared, stable Rc allocation; registration never creates an exclusive
//! reference to it. Sources and Luna are removed before that allocation is freed.
//! The safe entry point has no caller-side memory-safety preconditions; these
//! native invariants are the wrapper's responsibility, never its caller's.
use crate::status::{Status, subscription_request};
use anyhow::{Context as _, Result, anyhow, ensure};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_void},
    io::Write,
    mem::MaybeUninit,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr::{self, NonNull},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all,
    clippy::pedantic,
    clippy::undocumented_unsafe_blocks
)]
mod ffi {
    pub mod luna {
        include!("generated/luna.rs");
        #[cfg(test)]
        include!("luna/tests/luna_api.rs");
    }
    pub mod glib {
        include!("generated/glib.rs");
        #[cfg(test)]
        include!("luna/tests/glib_api.rs");
    }
}
use ffi::{
    glib::Glib,
    luna::{LSHandle, LSMessage, LSMethod, Luna},
};

// Only Service owns this Rc. Raw callback pointers never acquire ownership.
// Rc and RefCell also prevent Send/Sync implementations from being inferred.
struct Callbacks {
    luna: Luna,
    glib: Glib,
    handle: NonNull<LSHandle>,
    context: NonNull<ffi::glib::GMainContext>,
    main_loop: NonNull<ffi::glib::GMainLoop>,
    status: RefCell<Status>,
    stop: Arc<AtomicBool>,
    deadline: Instant,
}

struct Service {
    callbacks: Rc<Callbacks>,
    methods: Box<[LSMethod; 2]>,
    timer: Option<NonNull<ffi::glib::GSource>>,
    power_token: ffi::luna::LSMessageToken,
}

pub(super) fn run(name: &str, lifetime: Duration, stop: Arc<AtomicBool>) -> Result<()> {
    // SAFETY: load TV libraries with signatures generated from the matching SDK.
    let luna = unsafe { Luna::new("/usr/lib/libluna-service2.so.3") }.context("load Luna")?;
    // SAFETY: generated table retains the library for every function pointer's lifetime.
    let glib = unsafe { Glib::new("/usr/lib/libglib-2.0.so.0") }.context("load GLib")?;
    let mut service = Service::new(luna, glib, name, lifetime, stop)?;
    service.register()?;
    eprintln!("native Luna proof registered: {name}");
    let state = &service.callbacks;
    // SAFETY: this private loop is live and is only driven here, on its owner thread.
    unsafe {
        state.glib.g_main_loop_run(state.main_loop.as_ptr());
    }
    Ok(())
}

impl Service {
    fn new(
        luna: Luna,
        glib: Glib,
        name: &str,
        lifetime: Duration,
        stop: Arc<AtomicBool>,
    ) -> Result<Self> {
        let name = CString::new(name)?;
        let deadline = Instant::now()
            .checked_add(lifetime)
            .context("invalid service lifetime")?;
        // SAFETY: create an owned, private context, never attached to a default loop.
        let context =
            NonNull::new(unsafe { glib.g_main_context_new() }).context("create GLib context")?;
        // SAFETY: context is live; the loop takes its own reference.
        let main_loop = NonNull::new(unsafe { glib.g_main_loop_new(context.as_ptr(), 0) });
        let Some(main_loop) = main_loop else {
            // SAFETY: release the context reference we own on this failure path.
            unsafe {
                glib.g_main_context_unref(context.as_ptr());
            }
            return Err(anyhow!("create GLib loop"));
        };
        let mut handle = ptr::null_mut();
        let registration = check(&luna, |error| {
            // SAFETY: name is NUL terminated and output points to local initialized storage.
            unsafe { luna.LSRegister(name.as_ptr(), &raw mut handle, error) }
        });
        if let Err(error) = registration {
            // SAFETY: no registration or callbacks exist; release both owned references.
            unsafe {
                glib.g_main_loop_unref(main_loop.as_ptr());
                glib.g_main_context_unref(context.as_ptr());
            }
            return Err(error);
        }
        // A successful LSRegister guarantees a live handle. Abort on a broken ABI
        // rather than passing a null handle into subsequent native operations.
        let handle = NonNull::new(handle).unwrap_or_else(|| std::process::abort());
        Ok(Self {
            callbacks: Rc::new(Callbacks {
                luna,
                glib,
                handle,
                context,
                main_loop,
                status: RefCell::new(Status::default()),
                stop,
                deadline,
            }),
            methods: Box::new([
                LSMethod {
                    name: c"status".as_ptr(),
                    function: Some(status_callback),
                    flags: 0,
                },
                LSMethod {
                    name: ptr::null(),
                    function: None,
                    flags: 0,
                },
            ]),
            timer: None,
            power_token: 0,
        })
    }

    fn register(&mut self) -> Result<()> {
        ensure!(
            self.power_token == 0 && self.timer.is_none(),
            "service already initialized"
        );
        let state = &self.callbacks;
        let data = Rc::as_ptr(state).cast_mut().cast::<c_void>();
        check(&state.luna, |error| {
            // SAFETY: the sentinel-terminated method table outlives the registration.
            unsafe {
                state.luna.LSRegisterCategory(
                    state.handle.as_ptr(),
                    c"/".as_ptr(),
                    self.methods.as_mut_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    error,
                )
            }
        })?;
        check(&state.luna, |error| {
            // SAFETY: shared Rc storage remains alive until LSUnregister completes.
            unsafe {
                state
                    .luna
                    .LSCategorySetData(state.handle.as_ptr(), c"/".as_ptr(), data, error)
            }
        })?;
        check(&state.luna, |error| {
            // SAFETY: handle and private loop are live, and neither crosses threads.
            unsafe {
                state
                    .luna
                    .LSGmainAttach(state.handle.as_ptr(), state.main_loop.as_ptr(), error)
            }
        })?;
        check(&state.luna, |error| {
            // SAFETY: payload/URI are static; callback storage remains shared and live
            // through cancellation and unregistration, including partial initialization.
            unsafe {
                state.luna.LSCall(
                    state.handle.as_ptr(),
                    c"luna://com.webos.service.tvpower/power/getPowerState".as_ptr(),
                    c"{\"subscribe\":true}".as_ptr(),
                    Some(power_callback),
                    data,
                    &raw mut self.power_token,
                    error,
                )
            }
        })?;
        // SAFETY: create a caller-owned timer source; retain it even if attach fails.
        let timer = NonNull::new(unsafe { state.glib.g_timeout_source_new(100) })
            .context("create shutdown timer")?;
        self.timer = Some(timer);
        // SAFETY: source and data are live. No destroy callback owns the Rc allocation.
        unsafe {
            state
                .glib
                .g_source_set_callback(timer.as_ptr(), Some(timer_callback), data, None);
        }
        // SAFETY: attach only to our private context, never the global default context.
        let id = unsafe {
            state
                .glib
                .g_source_attach(timer.as_ptr(), state.context.as_ptr())
        };
        ensure!(id != 0, "attach shutdown timer");
        Ok(())
    }
}

impl Callbacks {
    fn snapshot(&self, subscribed: bool) -> Value {
        // SAFETY: called only while registered; key is a static C string.
        let count = unsafe {
            self.luna
                .LSSubscriptionGetHandleSubscribersCount(self.handle.as_ptr(), c"/status".as_ptr())
        };
        self.status.borrow().snapshot(subscribed, count)
    }

    fn status(&self, message: &Message<'_>) -> Result<()> {
        let result = message
            .payload()
            .and_then(|value| subscription_request(&value))
            .and_then(|subscribe| {
                let subscribed = subscribe && message.subscribe()?;
                Ok(self.snapshot(subscribed))
            });
        let reply = result
            .unwrap_or_else(|error| json!({"returnValue": false, "errorText": error.to_string()}));
        message.respond(&reply)
    }

    fn power(&self, message: &Message<'_>) -> Result<()> {
        let value = message.payload()?;
        let changed = self.status.borrow_mut().observe_power(&value);
        if changed {
            let payload = CString::new(self.snapshot(true).to_string())?;
            check(&self.luna, |error| {
                // SAFETY: LS2 copies JSON during this call; handle and key are live.
                unsafe {
                    self.luna.LSSubscriptionReply(
                        self.handle.as_ptr(),
                        c"/status".as_ptr(),
                        payload.as_ptr(),
                        error,
                    )
                }
            })?;
        }
        Ok(())
    }
}

/// A non-owning message, borrowed only for the native callback invocation.
struct Message<'callback> {
    state: &'callback Callbacks,
    raw: NonNull<LSMessage>,
}

impl<'callback> Message<'callback> {
    /// # Safety
    /// `raw` must be LS2's live message for this callback and this service. The
    /// chosen lifetime must not extend beyond the callback; no pointer is retained.
    unsafe fn from_callback(state: &'callback Callbacks, raw: *mut LSMessage) -> Result<Self> {
        Ok(Self {
            state,
            raw: NonNull::new(raw).context("missing Luna message")?,
        })
    }

    fn payload(&self) -> Result<Value> {
        // SAFETY: constructor guarantees a live callback message from this service.
        let text = unsafe { self.state.luna.LSMessageGetPayload(self.raw.as_ptr()) };
        ensure!(!text.is_null(), "missing Luna payload");
        // SAFETY: LS2 owns this NUL-terminated buffer through the callback. Only the
        // parsed, owned JSON leaves this method; no C string reference can escape.
        let text = unsafe { CStr::from_ptr(text) };
        ensure!(text.to_bytes().len() <= 4096, "oversized Luna payload");
        Ok(serde_json::from_slice(text.to_bytes())?)
    }

    fn respond(&self, value: &Value) -> Result<()> {
        let payload = CString::new(serde_json::to_string(value)?)?;
        check(&self.state.luna, |error| {
            // SAFETY: message is live; LS2 copies JSON before this call returns.
            unsafe {
                self.state.luna.LSMessageReply(
                    self.state.handle.as_ptr(),
                    self.raw.as_ptr(),
                    payload.as_ptr(),
                    error,
                )
            }
        })
    }

    fn subscribe(&self) -> Result<bool> {
        let mut subscribed = false;
        check(&self.state.luna, |error| {
            // SAFETY: LS2 retains the native message itself if subscribed. The Rust
            // wrapper is never stored, and LS2 releases its reference on cancellation.
            unsafe {
                self.state.luna.LSSubscriptionProcess(
                    self.state.handle.as_ptr(),
                    self.raw.as_ptr(),
                    &raw mut subscribed,
                    error,
                )
            }
        })?;
        Ok(subscribed)
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // The loop has stopped (or never ran). The Rc pointee remains shared even
        // if cancellation synchronously calls back; cleanup never takes &mut Callbacks.
        let state = &self.callbacks;
        if let Some(timer) = self.timer.take() {
            // SAFETY: destroy detaches the source before releasing our owned reference.
            unsafe {
                state.glib.g_source_destroy(timer.as_ptr());
                state.glib.g_source_unref(timer.as_ptr());
            }
        }
        if self.power_token != 0
            && let Err(error) = check(&state.luna, |error| {
                // SAFETY: live handle and the token written by this service's LSCall.
                unsafe {
                    state
                        .luna
                        .LSCallCancel(state.handle.as_ptr(), self.power_token, error)
                }
            })
        {
            let _ = writeln!(std::io::stderr(), "cancel power subscription: {error}");
        }
        if let Err(error) = check(&state.luna, |error| {
            // SAFETY: handle is owned by Service. This removes all LS2 callback
            // sources before Rc storage, method table or libraries can be dropped.
            unsafe { state.luna.LSUnregister(state.handle.as_ptr(), error) }
        }) {
            let _ = writeln!(std::io::stderr(), "unregister Luna: {error}");
            // LS2 may retain callback pointers on failure: never free their storage.
            std::process::abort();
        }
    }
}

impl Drop for Callbacks {
    fn drop(&mut self) {
        // SAFETY: Service removed all native callbacks before dropping its Rc.
        // Release the loop's context reference, then our context reference.
        unsafe {
            self.glib.g_main_loop_unref(self.main_loop.as_ptr());
            self.glib.g_main_context_unref(self.context.as_ptr());
        }
    }
}

struct Error<'api> {
    api: &'api Luna,
    raw: ffi::luna::LSError,
}

impl<'api> Error<'api> {
    fn new(api: &'api Luna) -> Result<Self> {
        let mut raw = MaybeUninit::uninit();
        // SAFETY: aligned storage for the SDK-generated LSError, fully initialized
        // by LSErrorInit (memset + magic). Never read it if initialization fails.
        let initialized = unsafe { api.LSErrorInit(raw.as_mut_ptr()) };
        ensure!(initialized, "initialize Luna error");
        // SAFETY: all fields have been initialized by the successful native call.
        let raw = unsafe { raw.assume_init() };
        Ok(Self { api, raw })
    }
}

impl Drop for Error<'_> {
    fn drop(&mut self) {
        // SAFETY: one owner, initialized error; free once, including Rust unwind paths.
        unsafe {
            self.api.LSErrorFree(&raw mut self.raw);
        }
    }
}

fn check(luna: &Luna, action: impl FnOnce(*mut ffi::luna::LSError) -> bool) -> Result<()> {
    let mut error = Error::new(luna)?;
    if action(&raw mut error.raw) {
        return Ok(());
    }
    let message = if error.raw.message.is_null() {
        "Luna operation failed".into()
    } else {
        // SAFETY: initialized LSError owns this NUL-terminated message until Drop.
        unsafe { CStr::from_ptr(error.raw.message) }
            .to_string_lossy()
            .into_owned()
    };
    Err(anyhow!("{message} (code {})", error.raw.error_code))
}

fn callback(action: impl FnOnce() -> Result<()>) -> bool {
    // Catch the entire callback, including diagnostics and destructors. Never
    // resume after a panic in potentially partially updated state or unwind into C.
    match catch_unwind(AssertUnwindSafe(|| {
        if let Err(error) = action() {
            let _ = writeln!(std::io::stderr(), "Luna callback: {error:#}");
        }
    })) {
        Ok(()) => true,
        Err(payload) => {
            // Even a panic payload's destructor may panic; do not run it across C.
            std::mem::forget(payload);
            std::process::abort();
        }
    }
}

/// # Safety
/// `data` is the live shared Rc pointee installed for this category, and
/// `message` is borrowed from LS2 for this invocation on the private `GLib` thread.
unsafe extern "C" fn status_callback(
    _: *mut LSHandle,
    message: *mut LSMessage,
    data: *mut c_void,
) -> bool {
    callback(|| {
        // SAFETY: native registration guarantees stable, shared callback storage.
        let state =
            unsafe { data.cast::<Callbacks>().as_ref() }.context("missing callback state")?;
        // SAFETY: wrapper stays local to this callback and never retains the message.
        let message = unsafe { Message::from_callback(state, message) }?;
        state.status(&message)
    })
}

/// # Safety
/// Same lifetime/thread contract as `status_callback`, installed by `LSCall`.
unsafe extern "C" fn power_callback(
    _: *mut LSHandle,
    message: *mut LSMessage,
    data: *mut c_void,
) -> bool {
    callback(|| {
        // SAFETY: Service retains this shared Rc pointee until cancellation/unregistration.
        let state =
            unsafe { data.cast::<Callbacks>().as_ref() }.context("missing callback state")?;
        // SAFETY: LS2 message is borrowed only for this invocation.
        let message = unsafe { Message::from_callback(state, message) }?;
        state.power(&message)
    })
}

/// # Safety
/// `data` is the live shared Rc pointee; Service destroys this private-context
/// source before freeing that storage. `GLib` invokes it only on the run thread.
unsafe extern "C" fn timer_callback(data: *mut c_void) -> i32 {
    callback(|| {
        // SAFETY: private source retains this non-owning pointer while Service owns the Rc.
        let state =
            unsafe { data.cast::<Callbacks>().as_ref() }.context("missing callback state")?;
        if state.stop.load(Ordering::Relaxed) || Instant::now() >= state.deadline {
            // SAFETY: live loop owned by Service, running on this thread.
            unsafe {
                state.glib.g_main_loop_quit(state.main_loop.as_ptr());
            }
        }
        Ok(())
    });
    1
}

#[cfg(test)]
mod tests;
