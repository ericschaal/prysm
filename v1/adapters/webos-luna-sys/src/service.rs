//! A registered Luna service on a private GLib context. No raw pointer or
//! native type leaves this module.
//!
//! Callback state ([`Callbacks`]) is one `Rc` allocation registered with LS2
//! as category data and as each call's user data (through its [`Call`]
//! slot). It is only ever shared, never borrowed mutably, and freed after
//! `LSUnregister` removed every native reference. The handler sits in a
//! `RefCell`; LS2 is never called while it is borrowed (bus actions are
//! deferred), and a re-entrant callback that finds it borrowed is dropped.
use crate::generated::glib::{self, Glib};
use crate::generated::luna::{self as ffi, LSHandle, LSMessage, LSMethod, Luna};
use crate::{Error, GLIB_LIBRARY, LUNA_LIBRARY};
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString, c_char, c_void};
use std::io::Write;
use std::mem::MaybeUninit;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Largest request payload passed to the handler.
pub const MAX_PAYLOAD: usize = 4096;

/// One method call to this service.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    /// LS2's application id of the caller (or its service name); never
    /// taken from the payload.
    pub sender: &'a str,
    pub payload: &'a str,
}

/// The reply to a [`Request`]. With `subscribe`, the caller is added to the
/// subscribers of `/<method>` before the reply goes out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub payload: String,
    pub subscribe: bool,
}

/// What a handler asks of the bus. Carried out after the handler returns.
#[derive(Debug, Default)]
pub struct Bus {
    publishes: Vec<(String, String)>,
    recalls: Vec<usize>,
}

impl Bus {
    /// Send `payload` to every subscriber of `key` (such as `/getState`).
    pub fn publish(&mut self, key: &str, payload: &str) {
        self.publishes.push((key.into(), payload.into()));
    }

    /// Cancel call `index` of [`Config::calls`] and make it again.
    pub fn recall(&mut self, index: usize) {
        self.recalls.push(index);
    }
}

/// The service's behaviour. Runs on the loop thread only.
pub trait Handler {
    fn request(&mut self, bus: &mut Bus, request: Request<'_>) -> Response;
    /// A reply to call `index` of [`Config::calls`].
    fn reply(&mut self, bus: &mut Bus, index: usize, payload: &str);
    /// Called every [`Config::tick`]. `false` ends [`run`].
    fn tick(&mut self, bus: &mut Bus, now: Instant) -> bool;
}

#[derive(Debug, Clone)]
pub struct Config<'a> {
    /// The service name to register, such as `io.prysm.tv.service`.
    pub name: &'a str,
    /// Methods in category `/`.
    pub methods: &'a [&'a str],
    /// `(uri, payload)` calls kept open, normally `{"subscribe":true}`.
    pub calls: &'a [(&'a str, &'a str)],
    pub tick: Duration,
}

/// Register, run the loop until the handler's `tick` returns `false`, then
/// cancel calls and unregister.
///
/// # Errors
/// [`Error::Unsupported`] off webOS, a library that cannot be loaded, or a
/// registration LS2 refuses.
pub fn run(config: &Config<'_>, handler: Box<dyn Handler>) -> Result<(), Error> {
    if !cfg!(target_os = "linux") {
        return Err(Error::Unsupported(
            "the Luna service bus exists only on an LG webOS TV",
        ));
    }
    // SAFETY: signatures generated from the matching TV SDK; each table keeps
    // its library loaded for as long as its function pointers exist.
    let luna = unsafe { Luna::new(LUNA_LIBRARY) }.map_err(|source| Error::Load {
        library: LUNA_LIBRARY.into(),
        source,
    })?;
    // SAFETY: as above.
    let glib = unsafe { Glib::new(GLIB_LIBRARY) }.map_err(|source| Error::Load {
        library: GLIB_LIBRARY.into(),
        source,
    })?;
    run_with(luna, glib, config, handler)
}

pub(crate) fn run_with(
    luna: Luna,
    glib: Glib,
    config: &Config<'_>,
    handler: Box<dyn Handler>,
) -> Result<(), Error> {
    let mut service = Service::new(luna, glib, config, handler)?;
    service.register(config.tick)?;
    let state = &service.callbacks;
    // SAFETY: the private loop is live and only driven here, on its owner thread.
    unsafe {
        state.glib.g_main_loop_run(state.main_loop.as_ptr());
    }
    Ok(())
}

/// One kept call. Its address is the call's user data.
struct Call {
    /// The owning callbacks; set once right after the `Rc` exists.
    owner: Cell<*const Callbacks>,
    index: usize,
    uri: CString,
    payload: CString,
    token: Cell<ffi::LSMessageToken>,
}

struct Callbacks {
    luna: Luna,
    glib: Glib,
    handle: NonNull<LSHandle>,
    context: NonNull<glib::GMainContext>,
    main_loop: NonNull<glib::GMainLoop>,
    handler: RefCell<Box<dyn Handler>>,
    calls: Box<[Call]>,
    closing: Cell<bool>,
}

struct Service {
    callbacks: Rc<Callbacks>,
    methods: Box<[LSMethod]>,
    _names: Box<[CString]>,
    timer: Option<NonNull<glib::GSource>>,
}

impl Service {
    fn new(
        luna: Luna,
        glib: Glib,
        config: &Config<'_>,
        handler: Box<dyn Handler>,
    ) -> Result<Self, Error> {
        let name = CString::new(config.name).map_err(|_| Error::Nul)?;
        let names = config
            .methods
            .iter()
            .map(|method| CString::new(*method).map_err(|_| Error::Nul))
            .collect::<Result<Box<[_]>, _>>()?;
        let calls = config
            .calls
            .iter()
            .enumerate()
            .map(|(index, (uri, payload))| {
                Ok(Call {
                    owner: Cell::new(ptr::null()),
                    index,
                    uri: CString::new(*uri).map_err(|_| Error::Nul)?,
                    payload: CString::new(*payload).map_err(|_| Error::Nul)?,
                    token: Cell::new(0),
                })
            })
            .collect::<Result<Box<[_]>, Error>>()?;
        // SAFETY: an owned, private context, never attached to a default loop.
        let context = NonNull::new(unsafe { glib.g_main_context_new() })
            .ok_or(Error::Glib("cannot create a GLib context"))?;
        // SAFETY: the context is live; the loop takes its own reference.
        let Some(main_loop) = NonNull::new(unsafe { glib.g_main_loop_new(context.as_ptr(), 0) })
        else {
            // SAFETY: release the one context reference we own.
            unsafe { glib.g_main_context_unref(context.as_ptr()) };
            return Err(Error::Glib("cannot create a GLib loop"));
        };
        let mut handle = ptr::null_mut();
        let registered = check(&luna, "LSRegister", |error| {
            // SAFETY: NUL-terminated name and a writable local handle.
            unsafe { luna.LSRegister(name.as_ptr(), &raw mut handle, error) }
        });
        if let Err(error) = registered {
            // SAFETY: nothing is registered; release both references we own.
            unsafe {
                glib.g_main_loop_unref(main_loop.as_ptr());
                glib.g_main_context_unref(context.as_ptr());
            }
            return Err(error);
        }
        // A successful LSRegister yields a live handle; a null one is a broken ABI.
        let handle = NonNull::new(handle).unwrap_or_else(|| std::process::abort());
        let callbacks = Rc::new(Callbacks {
            luna,
            glib,
            handle,
            context,
            main_loop,
            handler: RefCell::new(handler),
            calls,
            closing: Cell::new(false),
        });
        for call in &callbacks.calls {
            call.owner.set(Rc::as_ptr(&callbacks));
        }
        let methods = names
            .iter()
            .map(|name| LSMethod {
                name: name.as_ptr(),
                function: Some(method_callback),
                flags: 0,
            })
            .chain(std::iter::once(LSMethod {
                name: ptr::null(),
                function: None,
                flags: 0,
            }))
            .collect();
        Ok(Self {
            callbacks,
            methods,
            _names: names,
            timer: None,
        })
    }

    fn register(&mut self, tick: Duration) -> Result<(), Error> {
        let state = &self.callbacks;
        let data = Rc::as_ptr(state).cast_mut().cast::<c_void>();
        check(&state.luna, "LSRegisterCategory", |error| {
            // SAFETY: the sentinel-terminated table and its names outlive the registration.
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
        check(&state.luna, "LSCategorySetData", |error| {
            // SAFETY: the shared Rc pointee stays alive until LSUnregister completes.
            unsafe {
                state
                    .luna
                    .LSCategorySetData(state.handle.as_ptr(), c"/".as_ptr(), data, error)
            }
        })?;
        check(&state.luna, "LSGmainAttach", |error| {
            // SAFETY: handle and private loop are live and never cross threads.
            unsafe {
                state
                    .luna
                    .LSGmainAttach(state.handle.as_ptr(), state.main_loop.as_ptr(), error)
            }
        })?;
        for index in 0..state.calls.len() {
            // A failed call is retried when the handler recalls it.
            if let Err(error) = state.call(index) {
                log(&format!(
                    "cannot call {:?}: {error}",
                    state.calls[index].uri
                ));
            }
        }
        let interval = u32::try_from(tick.as_millis()).unwrap_or(u32::MAX).max(1);
        // SAFETY: a caller-owned timer source; kept even if attaching fails.
        let timer = NonNull::new(unsafe { state.glib.g_timeout_source_new(interval) })
            .ok_or(Error::Glib("cannot create the tick timer"))?;
        self.timer = Some(timer);
        // SAFETY: source and data are live; no destroy notify owns the Rc pointee.
        unsafe {
            state
                .glib
                .g_source_set_callback(timer.as_ptr(), Some(timer_callback), data, None);
        }
        // SAFETY: attach only to our private context, never the global default.
        let id = unsafe {
            state
                .glib
                .g_source_attach(timer.as_ptr(), state.context.as_ptr())
        };
        if id == 0 {
            return Err(Error::Glib("cannot attach the tick timer"));
        }
        Ok(())
    }
}

impl Callbacks {
    fn call(&self, index: usize) -> Result<(), Error> {
        let call = &self.calls[index];
        let mut token = 0;
        check(&self.luna, "LSCall", |error| {
            // SAFETY: live handle and strings; the user data is this call's slot,
            // which lives in the shared allocation until LSUnregister.
            unsafe {
                self.luna.LSCall(
                    self.handle.as_ptr(),
                    call.uri.as_ptr(),
                    call.payload.as_ptr(),
                    Some(reply_callback),
                    ptr::from_ref(call).cast_mut().cast(),
                    &raw mut token,
                    error,
                )
            }
        })?;
        call.token.set(token);
        Ok(())
    }

    fn cancel(&self, index: usize) -> Result<(), Error> {
        let call = &self.calls[index];
        let token = call.token.get();
        if token != 0 {
            check(&self.luna, "LSCallCancel", |error| {
                // SAFETY: the token was written by this handle's LSCall.
                unsafe { self.luna.LSCallCancel(self.handle.as_ptr(), token, error) }
            })?;
            call.token.set(0);
        }
        Ok(())
    }

    fn apply(&self, bus: Bus) {
        for (key, payload) in bus.publishes {
            let published = (|| {
                let key = CString::new(key).map_err(|_| Error::Nul)?;
                let payload = CString::new(payload).map_err(|_| Error::Nul)?;
                check(&self.luna, "LSSubscriptionReply", |error| {
                    // SAFETY: registered handle; LS2 copies both strings before returning.
                    unsafe {
                        self.luna.LSSubscriptionReply(
                            self.handle.as_ptr(),
                            key.as_ptr(),
                            payload.as_ptr(),
                            error,
                        )
                    }
                })
            })();
            if let Err(error) = published {
                log(&format!("cannot publish: {error}"));
            }
        }
        for index in bus.recalls {
            if index >= self.calls.len() {
                continue;
            }
            if let Err(error) = self.cancel(index).and_then(|()| self.call(index)) {
                log(&format!(
                    "cannot renew {:?}: {error}",
                    self.calls[index].uri
                ));
            }
        }
    }

    fn method(&self, raw: *mut LSMessage) -> Result<(), Error> {
        if self.closing.get() {
            return Ok(());
        }
        let message = Message {
            state: self,
            raw: NonNull::new(raw).ok_or(Error::Glib("missing Luna message"))?,
        };
        let method = message.method()?;
        let handled = (|| {
            let sender = message.sender()?;
            let payload = message.payload()?;
            let mut bus = Bus::default();
            let mut handler = self
                .handler
                .try_borrow_mut()
                .map_err(|_| "the service is busy".to_owned())?;
            let response = handler.request(
                &mut bus,
                Request {
                    method: &method,
                    sender: &sender,
                    payload: &payload,
                },
            );
            Ok::<_, String>((response, bus))
        })();
        match handled {
            Ok((response, bus)) => {
                if response.subscribe {
                    message.subscribe()?;
                }
                message.respond(&response.payload)?;
                self.apply(bus);
                Ok(())
            }
            Err(text) => message.respond(&error_reply(&text)),
        }
    }

    fn reply(&self, index: usize, raw: *mut LSMessage) -> Result<(), Error> {
        if self.closing.get() {
            return Ok(());
        }
        let message = Message {
            state: self,
            raw: NonNull::new(raw).ok_or(Error::Glib("missing Luna message"))?,
        };
        // An unreadable reply still counts as a (bad) reply.
        let payload = message.payload().unwrap_or_default();
        let mut bus = Bus::default();
        let Ok(mut handler) = self.handler.try_borrow_mut() else {
            return Ok(());
        };
        handler.reply(&mut bus, index, &payload);
        drop(handler);
        self.apply(bus);
        Ok(())
    }

    fn tick(&self) {
        if self.closing.get() {
            return;
        }
        let mut bus = Bus::default();
        let Ok(mut handler) = self.handler.try_borrow_mut() else {
            return;
        };
        let keep = handler.tick(&mut bus, Instant::now());
        drop(handler);
        self.apply(bus);
        if !keep {
            // SAFETY: the private loop is live on this owner thread.
            unsafe { self.glib.g_main_loop_quit(self.main_loop.as_ptr()) };
        }
    }
}

/// A message borrowed from LS2 for one callback invocation.
struct Message<'callback> {
    state: &'callback Callbacks,
    raw: NonNull<LSMessage>,
}

impl Message<'_> {
    fn method(&self) -> Result<String, Error> {
        // SAFETY: LS2's live message for this callback.
        let text = unsafe { self.state.luna.LSMessageGetMethod(self.raw.as_ptr()) };
        // SAFETY: LS2 owns the string for the callback's duration.
        unsafe { native_text(text) }
    }

    fn sender(&self) -> Result<String, String> {
        // SAFETY: identity fields come from LS2, never from the payload.
        let app = unsafe { self.state.luna.LSMessageGetApplicationID(self.raw.as_ptr()) };
        // SAFETY: LS2 owns this buffer for the callback's duration.
        let app = unsafe { native_text(app) }.map_err(|error| error.to_string())?;
        if !app.is_empty() {
            return Ok(app.split(' ').next().unwrap_or_default().to_owned());
        }
        // SAFETY: same live message; anonymous callers may return null.
        let service = unsafe {
            self.state
                .luna
                .LSMessageGetSenderServiceName(self.raw.as_ptr())
        };
        // SAFETY: as above.
        unsafe { native_text(service) }.map_err(|error| error.to_string())
    }

    fn payload(&self) -> Result<String, String> {
        // SAFETY: LS2's live message for this callback.
        let text = unsafe { self.state.luna.LSMessageGetPayload(self.raw.as_ptr()) };
        if text.is_null() {
            return Err("missing payload".into());
        }
        // SAFETY: LS2 owns this NUL-terminated buffer through the callback; only
        // an owned copy leaves this method.
        let text = unsafe { CStr::from_ptr(text) };
        if text.to_bytes().len() > MAX_PAYLOAD {
            return Err(format!("payload exceeds {MAX_PAYLOAD} bytes"));
        }
        text.to_str()
            .map(str::to_owned)
            .map_err(|_| "payload is not UTF-8".to_owned())
    }

    fn respond(&self, payload: &str) -> Result<(), Error> {
        let payload = CString::new(payload).map_err(|_| Error::Nul)?;
        check(&self.state.luna, "LSMessageReply", |error| {
            // SAFETY: live message; LS2 copies the payload before returning.
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

    fn subscribe(&self) -> Result<bool, Error> {
        let mut subscribed = false;
        check(&self.state.luna, "LSSubscriptionProcess", |error| {
            // SAFETY: LS2 takes its own reference to the message if it subscribes;
            // this wrapper is never stored.
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
/// A non-null `pointer` must be a live, NUL-terminated string owned by LS2
/// for the current callback.
unsafe fn native_text(pointer: *const c_char) -> Result<String, Error> {
    if pointer.is_null() {
        return Ok(String::new());
    }
    // SAFETY: guaranteed by the caller; an owned copy is returned.
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| Error::Glib("Luna returned non-UTF-8 text"))
}

fn error_reply(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match c {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            c if u32::from(c) < 0x20 => escaped.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => escaped.push(c),
        }
    }
    format!(r#"{{"returnValue":false,"errorText":"{escaped}"}}"#)
}

impl Drop for Service {
    fn drop(&mut self) {
        // The loop stopped (or never ran). Cleanup only shares the Rc pointee,
        // so a synchronous callback during cancellation still sees valid state.
        let state = &self.callbacks;
        state.closing.set(true);
        if let Some(timer) = self.timer.take() {
            // SAFETY: destroy detaches the source before our reference is released.
            unsafe {
                state.glib.g_source_destroy(timer.as_ptr());
                state.glib.g_source_unref(timer.as_ptr());
            }
        }
        for index in 0..state.calls.len() {
            if let Err(error) = state.cancel(index) {
                log(&format!("cannot cancel a call: {error}"));
            }
        }
        if let Err(error) = check(&state.luna, "LSUnregister", |error| {
            // SAFETY: the handle is ours. This removes every LS2 callback source
            // before the Rc pointee, the method table or the libraries are freed.
            unsafe { state.luna.LSUnregister(state.handle.as_ptr(), error) }
        }) {
            log(&format!("{error}"));
            // LS2 may still hold callback pointers: never free their storage.
            std::process::abort();
        }
    }
}

impl Drop for Callbacks {
    fn drop(&mut self) {
        // SAFETY: Service removed every native callback before dropping its Rc.
        // Release the loop's reference, then the context reference we own.
        unsafe {
            self.glib.g_main_loop_unref(self.main_loop.as_ptr());
            self.glib.g_main_context_unref(self.context.as_ptr());
        }
    }
}

/// An initialized `LSError`, freed exactly once.
pub(crate) struct LunaError<'api> {
    api: &'api Luna,
    raw: ffi::LSError,
}

impl<'api> LunaError<'api> {
    pub(crate) fn new(api: &'api Luna) -> Result<Self, Error> {
        let mut raw = MaybeUninit::uninit();
        // SAFETY: aligned storage for the SDK's LSError, fully initialized by a
        // successful LSErrorInit; never read otherwise.
        if !unsafe { api.LSErrorInit(raw.as_mut_ptr()) } {
            return Err(Error::Luna {
                operation: "LSErrorInit",
                message: "cannot initialize a Luna error".into(),
            });
        }
        // SAFETY: initialized by the successful call above.
        let raw = unsafe { raw.assume_init() };
        Ok(Self { api, raw })
    }

    pub(crate) fn raw(&mut self) -> *mut ffi::LSError {
        &raw mut self.raw
    }

    fn message(&self) -> String {
        if self.raw.message.is_null() {
            return format!("failed (code {})", self.raw.error_code);
        }
        // SAFETY: the initialized error owns this NUL-terminated message until Drop.
        let text = unsafe { CStr::from_ptr(self.raw.message) }.to_string_lossy();
        format!("{text} (code {})", self.raw.error_code)
    }
}

impl Drop for LunaError<'_> {
    fn drop(&mut self) {
        // SAFETY: one owner of an initialized error; freed once on every path.
        unsafe { self.api.LSErrorFree(&raw mut self.raw) };
    }
}

pub(crate) fn check(
    api: &Luna,
    operation: &'static str,
    action: impl FnOnce(*mut ffi::LSError) -> bool,
) -> Result<(), Error> {
    let mut error = LunaError::new(api)?;
    if action(error.raw()) {
        return Ok(());
    }
    Err(Error::Luna {
        operation,
        message: error.message(),
    })
}

fn log(text: &str) {
    let _ = writeln!(std::io::stderr(), "luna: {text}");
}

/// Run a whole callback; abort rather than unwind into C or continue after a
/// panic left state half-updated.
fn guarded(action: impl FnOnce() -> Result<(), Error>) {
    match catch_unwind(AssertUnwindSafe(|| {
        if let Err(error) = action() {
            log(&error.to_string());
        }
    })) {
        Ok(()) => {}
        Err(payload) => {
            // Even a payload's destructor may panic; do not run it.
            std::mem::forget(payload);
            std::process::abort();
        }
    }
}

/// # Safety
/// `data` is the live shared [`Callbacks`] registered as category data, and
/// `message` is borrowed from LS2 for this invocation on the loop thread.
unsafe extern "C" fn method_callback(
    _: *mut LSHandle,
    message: *mut LSMessage,
    data: *mut c_void,
) -> bool {
    guarded(|| {
        // SAFETY: registration guarantees stable, shared callback storage.
        let state = unsafe { data.cast::<Callbacks>().as_ref() }
            .ok_or(Error::Glib("missing callback state"))?;
        state.method(message)
    });
    true
}

/// # Safety
/// `data` is a [`Call`] slot inside the live shared [`Callbacks`], installed
/// by `LSCall`; same thread and lifetime contract as [`method_callback`].
unsafe extern "C" fn reply_callback(
    _: *mut LSHandle,
    message: *mut LSMessage,
    data: *mut c_void,
) -> bool {
    guarded(|| {
        // SAFETY: the slot lives in the shared allocation until LSUnregister.
        let call =
            unsafe { data.cast::<Call>().as_ref() }.ok_or(Error::Glib("missing call slot"))?;
        // SAFETY: set to the owning Rc pointee before any call was made.
        let state =
            unsafe { call.owner.get().as_ref() }.ok_or(Error::Glib("missing callback state"))?;
        state.reply(call.index, message)
    });
    true
}

/// # Safety
/// `data` is the live shared [`Callbacks`]; the source is destroyed before
/// that storage is freed. GLib calls it only on the loop thread.
unsafe extern "C" fn timer_callback(data: *mut c_void) -> i32 {
    guarded(|| {
        // SAFETY: the private source keeps this non-owning pointer while Service owns the Rc.
        let state = unsafe { data.cast::<Callbacks>().as_ref() }
            .ok_or(Error::Glib("missing callback state"))?;
        state.tick();
        Ok(())
    });
    1
}

#[cfg(test)]
mod tests;
