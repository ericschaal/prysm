//! LS2/GLib boundary. No raw pointer or native type leaves this module.
//!
//! A private `GLib` context dispatches callbacks on the run thread. Callback state
//! has a shared, stable Rc allocation; registration never creates an exclusive
//! reference to it. Sources and Luna are removed before that allocation is freed.
//! The safe entry point has no caller-side memory-safety preconditions; these
//! native invariants are the wrapper's responsibility, never its caller's.
use crate::status::{Status, subscription_request};
use anyhow::{Context as _, Result, anyhow, ensure};
use prysm_webos_protocol::worker::VideoAvailability;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
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
    luna: Rc<Luna>,
    glib: Glib,
    handle: NonNull<LSHandle>,
    context: NonNull<ffi::glib::GMainContext>,
    main_loop: NonNull<ffi::glib::GMainLoop>,
    status: RefCell<Status>,
    stop: Arc<AtomicBool>,
    deadline: Option<Instant>,
    app: RefCell<Option<crate::application::App>>,
    pending: RefCell<Option<PendingMessage>>,
    power_token: Cell<ffi::luna::LSMessageToken>,
    power_retry: Cell<Option<Instant>>,
    video_token: Cell<ffi::luna::LSMessageToken>,
    video_retry: Cell<Option<Instant>>,
    video_context: RefCell<Option<VideoContext>>,
    closing: Cell<bool>,
}

struct Service {
    callbacks: Rc<Callbacks>,
    methods: Box<[LSMethod]>,
    timer: Option<NonNull<ffi::glib::GSource>>,
}

pub(super) fn run(
    name: &str,
    lifetime: Option<Duration>,
    stop: Arc<AtomicBool>,
    app: Option<crate::application::App>,
) -> Result<()> {
    // SAFETY: load TV libraries with signatures generated from the matching SDK.
    let luna = unsafe { Luna::new("/usr/lib/libluna-service2.so.3") }.context("load Luna")?;
    // SAFETY: generated table retains the library for every function pointer's lifetime.
    let glib = unsafe { Glib::new("/usr/lib/libglib-2.0.so.0") }.context("load GLib")?;
    let mut service = Service::new(luna, glib, name, lifetime, stop)?;
    *service.callbacks.app.borrow_mut() = app;
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
        lifetime: Option<Duration>,
        stop: Arc<AtomicBool>,
    ) -> Result<Self> {
        let name = CString::new(name)?;
        let deadline = lifetime
            .map(|lifetime| {
                Instant::now()
                    .checked_add(lifetime)
                    .context("invalid service lifetime")
            })
            .transpose()?;
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
                luna: Rc::new(luna),
                glib,
                handle,
                context,
                main_loop,
                status: RefCell::new(Status::default()),
                stop,
                deadline,
                app: RefCell::new(None),
                pending: RefCell::new(None),
                power_token: Cell::new(0),
                power_retry: Cell::new(None),
                video_token: Cell::new(0),
                video_retry: Cell::new(None),
                video_context: RefCell::new(None),
                closing: Cell::new(false),
            }),
            methods: [c"status", c"start", c"stop", c"setConfig", c"cleanup"]
                .into_iter()
                .map(|name| LSMethod {
                    name: name.as_ptr(),
                    function: Some(status_callback),
                    flags: 0,
                })
                .chain(std::iter::once(LSMethod {
                    name: ptr::null(),
                    function: None,
                    flags: 0,
                }))
                .collect(),
            timer: None,
        })
    }

    fn register(&mut self) -> Result<()> {
        ensure!(
            self.callbacks.power_token.get() == 0 && self.timer.is_none(),
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
        if let Err(error) = state.subscribe_power(data) {
            state
                .status
                .borrow_mut()
                .observe_power(&json!({"returnValue":false,"errorText":error.to_string()}));
            if let Some(app) = state.app.borrow_mut().as_mut() {
                app.power(None, state.status.borrow().error(), Instant::now());
            }
        }
        let has_app = state.app.borrow().is_some();
        if has_app
            && state.subscribe_video(data).is_err()
            && let Some(app) = state.app.borrow_mut().as_mut()
        {
            app.video_available(VideoAvailability::Unknown);
        }
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
        let mut value = self.status.borrow().snapshot(subscribed, count);
        if let Some(app) = self.app.borrow().as_ref() {
            value.as_object_mut().expect("status object").extend(
                app.state
                    .snapshot()
                    .as_object()
                    .expect("supervisor object")
                    .clone(),
            );
        }
        value
    }

    fn status(&self, message: &Message<'_>) -> Result<()> {
        if self.closing.get() {
            return Ok(());
        }
        let method = message.method()?;
        let result = if method == "status" {
            message
                .payload()
                .and_then(|value| subscription_request(&value))
                .and_then(|subscribe| {
                    let subscribed = subscribe && message.subscribe()?;
                    Ok(Some(self.snapshot(subscribed)))
                })
        } else {
            (|| {
                let value = message.payload()?;
                let sender = message.sender()?;
                let mut app = self.app.borrow_mut();
                app.as_mut()
                    .context("Supervisor is disabled in read-only probe mode")?
                    .request(&method, value, &sender, Instant::now())
            })()
        };
        let changed = method != "status" && result.is_ok();
        let response = match result {
            Ok(Some(value)) => {
                let reply = if method == "status" || value["returnValue"] == false {
                    value
                } else {
                    self.snapshot(false)
                };
                message.respond(&reply)
            }
            Ok(None) => {
                *self.pending.borrow_mut() = Some(message.retain());
                Ok(())
            }
            Err(error) => {
                message.respond(&json!({"returnValue":false, "errorText":error.to_string()}))
            }
        };
        if changed {
            self.status.borrow_mut().bump();
            let published = self.publish();
            response.and(published)
        } else {
            response
        }
    }
    fn publish(&self) -> Result<()> {
        let payload = CString::new(self.snapshot(true).to_string())?;
        check(&self.luna, |error| {
            // SAFETY: registered handle and static key are live; LS2 copies JSON.
            unsafe {
                self.luna.LSSubscriptionReply(
                    self.handle.as_ptr(),
                    c"/status".as_ptr(),
                    payload.as_ptr(),
                    error,
                )
            }
        })
    }
    fn subscribe_power(&self, data: *mut c_void) -> Result<()> {
        let mut token = 0;
        self.power_retry.set(None);
        let result = check(&self.luna, |error| {
            // SAFETY: all strings are static, the output is local, and Service retains
            // this shared callback allocation until every call is cancelled/unregistered.
            unsafe {
                self.luna.LSCall(
                    self.handle.as_ptr(),
                    c"luna://com.webos.service.tvpower/power/getPowerState".as_ptr(),
                    c"{\"subscribe\":true}".as_ptr(),
                    Some(power_callback),
                    data,
                    &raw mut token,
                    error,
                )
            }
        });
        if result.is_err() {
            self.power_retry
                .set(Some(Instant::now() + Duration::from_secs(5)));
        }
        result?;
        self.power_token.set(token);
        Ok(())
    }
    fn subscribe_video(&self, data: *mut c_void) -> Result<()> {
        let mut token = 0;
        // Also bound a missing initial response; unknown state retains capture fallback.
        self.video_retry
            .set(Some(Instant::now() + Duration::from_secs(5)));
        check(&self.luna, |error| {
            // SAFETY: static strings, local output, and callback storage retained by Service.
            unsafe {
                self.luna.LSCall(
                    self.handle.as_ptr(),
                    c"luna://com.webos.service.videooutput/getStatus".as_ptr(),
                    c"{\"subscribe\":true}".as_ptr(),
                    Some(video_callback),
                    data,
                    &raw mut token,
                    error,
                )
            }
        })?;
        self.video_token.set(token);
        Ok(())
    }
    fn video(&self, message: &Message<'_>) {
        if self.closing.get() {
            return;
        }
        let value = message.payload().unwrap_or(Value::Null);
        let observation = VideoObservation::parse(&value);
        self.video_retry.set(
            if observation.availability != VideoAvailability::Unknown && value["subscribed"] == true
            {
                None
            } else {
                Some(Instant::now() + Duration::from_secs(5))
            },
        );
        if let Some(app) = self.app.borrow_mut().as_mut() {
            if let Some(context) = observation.context {
                let mut previous = self.video_context.borrow_mut();
                if previous.as_ref().is_some_and(|old| old != &context) {
                    app.reset_video();
                }
                *previous = Some(context);
            }
            app.video_available(observation.availability);
        }
    }
    fn tick(&self, now: Instant) -> Result<()> {
        if self
            .video_retry
            .get()
            .is_some_and(|deadline| now >= deadline)
        {
            self.video_retry.set(Some(now + Duration::from_secs(5)));
            if let Some(app) = self.app.borrow_mut().as_mut() {
                app.video_available(VideoAvailability::Unknown);
            }
            let token = self.video_token.get();
            let cancelled = token == 0
                || check(&self.luna, |error| {
                    // SAFETY: token belongs to our live registered handle.
                    unsafe { self.luna.LSCallCancel(self.handle.as_ptr(), token, error) }
                })
                .is_ok();
            if cancelled {
                self.video_token.set(0);
                let _ = self.subscribe_video(ptr::from_ref(self).cast_mut().cast());
            }
        }
        if self
            .power_retry
            .get()
            .is_some_and(|deadline| now >= deadline)
        {
            self.power_retry.set(Some(now + Duration::from_secs(5)));
            let token = self.power_token.get();
            let cancelled = token == 0
                || check(&self.luna, |error| {
                    // SAFETY: token belongs to this registered handle and remains owned here.
                    unsafe { self.luna.LSCallCancel(self.handle.as_ptr(), token, error) }
                })
                .is_ok();
            if cancelled {
                self.power_token.set(0);
                let _ = self.subscribe_power(ptr::from_ref(self).cast_mut().cast());
            }
        }
        let shutting_down = self.stop.load(Ordering::Relaxed)
            || self.deadline.is_some_and(|deadline| now >= deadline);
        let mut app = self.app.borrow_mut();
        let mut changed = false;
        let mut reply = None;
        let stopped = if let Some(app) = app.as_mut() {
            let before = app.state.snapshot();
            if shutting_down {
                app.shutdown(now);
            }
            reply = app.tick(now);
            changed = before != app.state.snapshot();
            app.stopped()
        } else {
            true
        };
        drop(app);
        let message = if reply.is_some() {
            self.pending.borrow_mut().take()
        } else {
            None
        };
        let response = if let (Some(value), Some(message)) = (reply, message) {
            let value = if value["returnValue"] == false {
                value
            } else {
                self.snapshot(false)
            };
            message.respond(self, &value)
        } else {
            Ok(())
        };
        let published = if changed {
            self.status.borrow_mut().bump();
            self.publish()
        } else {
            Ok(())
        };
        if shutting_down && stopped {
            // SAFETY: the private loop is live on this owner thread.
            unsafe {
                self.glib.g_main_loop_quit(self.main_loop.as_ptr());
            }
        }
        response.and(published)
    }

    fn power(&self, message: &Message<'_>) -> Result<()> {
        if self.closing.get() {
            return Ok(());
        }
        let value = message
            .payload()
            .unwrap_or_else(|error| json!({"returnValue":false,"errorText":error.to_string()}));
        let changed = self.status.borrow_mut().observe_power(&value);
        let (power, error) = {
            let status = self.status.borrow();
            (status.power(), status.error())
        };
        if error.is_some() && self.power_retry.get().is_none() {
            self.power_retry
                .set(Some(Instant::now() + Duration::from_secs(5)));
        }
        if let Some(app) = self.app.borrow_mut().as_mut() {
            app.power(power, error, Instant::now());
        }
        if changed {
            self.publish()?;
        }

        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct VideoContext {
    source: Value,
    port: Value,
    context: Value,
}

struct VideoObservation {
    availability: VideoAvailability,
    context: Option<VideoContext>,
}
impl VideoObservation {
    fn parse(value: &Value) -> Self {
        // No app ID or HDMI port filter: MAIN may carry any input or a built-in video app.
        let main = value["video"]
            .as_array()
            .and_then(|sinks| sinks.iter().find(|sink| sink["sink"] == "MAIN"))
            .filter(|_| value["returnValue"] == true);
        let available = main.and_then(|main| {
            if !main["connected"].as_bool()? {
                return Some(false);
            }
            // Muted video can be legitimate black; it must not trigger a restart loop.
            Some(
                main["width"].as_u64()? > 0
                    && main["height"].as_u64()? > 0
                    && main["frameRate"].as_f64()? > 0.0,
            )
        });
        let availability = match available {
            Some(true) => VideoAvailability::Available,
            Some(false) => VideoAvailability::Unavailable,
            None => VideoAvailability::Unknown,
        };
        let context = main
            .filter(|_| availability == VideoAvailability::Available)
            .map(|main| VideoContext {
                source: main["connectedSource"].clone(),
                port: main["connectedSourcePort"].clone(),
                context: main["context"].clone(),
            });
        Self {
            availability,
            context,
        }
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

    fn method(&self) -> Result<String> {
        // SAFETY: constructor guarantees this callback message is live.
        let pointer = unsafe { self.state.luna.LSMessageGetMethod(self.raw.as_ptr()) };
        // SAFETY: LS2 getter returned a callback-owned string.
        unsafe { native_text(pointer) }
    }
    fn sender(&self) -> Result<String> {
        // SAFETY: both identity fields are supplied by LS2, never by JSON payload.
        let app = unsafe { self.state.luna.LSMessageGetApplicationID(self.raw.as_ptr()) };
        // SAFETY: LS2 owns this identity buffer for the callback duration.
        let app = unsafe { native_text(app) }?;
        if !app.is_empty() {
            return Ok(app.split(' ').next().unwrap_or_default().to_owned());
        }
        // SAFETY: same live callback message; anonymous clients may return null.
        let service = unsafe {
            self.state
                .luna
                .LSMessageGetSenderServiceName(self.raw.as_ptr())
        };
        // SAFETY: same live callback identity or null for anonymous clients.
        unsafe { native_text(service) }
    }
    fn retain(&self) -> PendingMessage {
        // SAFETY: acquire one LS2 reference while the borrowed message is still live.
        unsafe {
            self.state.luna.LSMessageRef(self.raw.as_ptr());
        }
        PendingMessage {
            api: Rc::clone(&self.state.luna),
            raw: self.raw,
        }
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

/// # Safety
/// Non-null input must be a live LS2-owned, NUL-terminated callback string.
unsafe fn native_text(pointer: *const std::ffi::c_char) -> Result<String> {
    if pointer.is_null() {
        return Ok(String::new());
    }
    // SAFETY: only LS2 getter results enter this private helper; LS2 owns their
    // NUL-terminated strings for the callback duration. Return an owned copy.
    Ok(unsafe { CStr::from_ptr(pointer) }.to_str()?.to_owned())
}
struct PendingMessage {
    api: Rc<Luna>,
    raw: NonNull<LSMessage>,
}
impl PendingMessage {
    fn respond(&self, state: &Callbacks, value: &Value) -> Result<()> {
        Message {
            state,
            raw: self.raw,
        }
        .respond(value)
    }
}
impl Drop for PendingMessage {
    fn drop(&mut self) {
        // SAFETY: balances the single retain; Service clears pending replies before
        // unregistering the connection, while this Rc retains the function table.
        unsafe {
            self.api.LSMessageUnref(self.raw.as_ptr());
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // The loop has stopped (or never ran). The Rc pointee remains shared even
        // if cancellation synchronously calls back; cleanup never takes &mut Callbacks.
        let state = &self.callbacks;
        state.closing.set(true);
        if let Some(timer) = self.timer.take() {
            // SAFETY: destroy detaches the source before releasing our owned reference.
            unsafe {
                state.glib.g_source_destroy(timer.as_ptr());
                state.glib.g_source_unref(timer.as_ptr());
            }
        }
        let pending = state.pending.borrow_mut().take();
        drop(pending);
        if state.power_token.get() != 0
            && let Err(error) = check(&state.luna, |error| {
                // SAFETY: live handle and the token written by this service's LSCall.
                unsafe {
                    state
                        .luna
                        .LSCallCancel(state.handle.as_ptr(), state.power_token.get(), error)
                }
            })
        {
            let _ = writeln!(std::io::stderr(), "cancel power subscription: {error}");
        }
        if state.video_token.get() != 0
            && let Err(error) = check(&state.luna, |error| {
                // SAFETY: live handle and token owned by this service's video subscription.
                unsafe {
                    state
                        .luna
                        .LSCallCancel(state.handle.as_ptr(), state.video_token.get(), error)
                }
            })
        {
            let _ = writeln!(std::io::stderr(), "cancel video subscription: {error}");
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
/// Same lifetime/thread contract as `power_callback`, installed by `LSCall`.
unsafe extern "C" fn video_callback(
    _: *mut LSHandle,
    message: *mut LSMessage,
    data: *mut c_void,
) -> bool {
    callback(|| {
        // SAFETY: Service retains this callback allocation until cancellation/unregistration.
        let state =
            unsafe { data.cast::<Callbacks>().as_ref() }.context("missing callback state")?;
        // SAFETY: the LS2 message is borrowed only for this invocation.
        let message = unsafe { Message::from_callback(state, message) }?;
        state.video(&message);
        Ok(())
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
        state.tick(Instant::now())?;
        Ok(())
    });
    1
}

#[cfg(test)]
mod tests;
