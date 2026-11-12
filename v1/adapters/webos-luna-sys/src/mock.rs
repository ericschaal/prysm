//! Stand-ins for every LS2 and GLib function, recording calls per thread, so
//! the wrapper's ownership and ordering run on the development host.
#![allow(clippy::missing_safety_doc)]
use crate::generated::glib::{
    GDestroyNotify, GMainContext, GSource, GSourceFunc, gboolean, gpointer, guint,
};
use crate::generated::luna::{
    GMainLoop, LSError, LSFilterFunc, LSHandle, LSMessage, LSMessageToken, LSMethod,
    LSMethodFunction, LSProperty, LSSignal,
};
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_uint, c_void};
use std::ptr::NonNull;

type Script = Box<dyn FnOnce()>;

#[derive(Default)]
struct State {
    calls: Vec<String>,
    deny: Option<String>,
    method: LSMethodFunction,
    data: Option<*mut c_void>,
    lscalls: Vec<(LSFilterFunc, *mut c_void)>,
    timer: Option<(GSourceFunc, gpointer)>,
    quit: bool,
    script: Option<Script>,
}

thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }

fn record(call: impl Into<String>) {
    let call = call.into();
    STATE.with_borrow_mut(|state| state.calls.push(call));
}

pub fn reset() {
    STATE.with_borrow_mut(|state| *state = State::default());
}

pub fn calls() -> Vec<String> {
    STATE.with_borrow(|state| state.calls.clone())
}

pub fn deny_register(name: Option<&str>) {
    STATE.with_borrow_mut(|state| state.deny = name.map(str::to_owned));
}

/// Run `script` inside `g_main_loop_run`.
pub fn on_run(script: impl FnOnce() + 'static) {
    STATE.with_borrow_mut(|state| state.script = Some(Box::new(script)));
}

pub fn luna() -> crate::generated::luna::Luna {
    crate::generated::luna::Luna::test_api()
}

pub fn glib() -> crate::generated::glib::Glib {
    crate::generated::glib::Glib::test_api()
}

pub fn library() -> libloading::Library {
    libloading::os::unix::Library::this().into()
}

/// A message as LS2 would hand it to a callback.
pub struct FakeMessage {
    pub method: CString,
    pub app: CString,
    pub payload: CString,
}

impl FakeMessage {
    pub fn new(method: &str, app: &str, payload: &str) -> Self {
        Self {
            method: CString::new(method).unwrap(),
            app: CString::new(app).unwrap(),
            payload: CString::new(payload).unwrap(),
        }
    }
    fn raw(&mut self) -> *mut LSMessage {
        std::ptr::from_mut(self).cast()
    }
}

/// # Safety
/// `message` was created by [`FakeMessage::raw`].
unsafe fn fake<'a>(message: *mut LSMessage) -> &'a FakeMessage {
    // SAFETY: guaranteed by the caller.
    unsafe { &*message.cast::<FakeMessage>() }
}

/// Deliver a method call to the registered category.
pub fn invoke_method(message: &mut FakeMessage) {
    let (method, data) = STATE.with_borrow(|state| (state.method, state.data));
    let method = method.expect("a registered method");
    // SAFETY: the wrapper registered this callback with this category data.
    unsafe { method(NonNull::dangling().as_ptr(), message.raw(), data.unwrap()) };
}

/// Deliver a reply to call `index`.
pub fn invoke_call(index: usize, payload: &str) {
    let (callback, data) = STATE.with_borrow(|state| state.lscalls[index]);
    let mut message = FakeMessage::new("", "", payload);
    // SAFETY: the wrapper passed this callback and user data to LSCall.
    unsafe { callback.unwrap()(NonNull::dangling().as_ptr(), message.raw(), data) };
}

/// Fire the timer; `true` once the loop was asked to quit.
pub fn invoke_timer() -> bool {
    let (callback, data) = STATE.with_borrow(|state| state.timer.expect("a timer"));
    // SAFETY: the wrapper installed this callback and data on the source.
    unsafe { callback.unwrap()(data) };
    STATE.with_borrow(|state| state.quit)
}

/// # Safety
/// `text` is a live C string.
unsafe fn text(text: *const c_char) -> String {
    // SAFETY: guaranteed by the caller.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

pub unsafe extern "C" fn error_init(error: *mut LSError) -> bool {
    record("init");
    // SAFETY: the caller supplies writable LSError storage; all-zero is valid.
    unsafe { error.write(std::mem::zeroed()) };
    true
}
pub unsafe extern "C" fn error_free(_: *mut LSError) {
    record("free");
}
pub unsafe extern "C" fn register(
    name: *const c_char,
    handle: *mut *mut LSHandle,
    error: *mut LSError,
) -> bool {
    // SAFETY: the wrapper passes a NUL-terminated name.
    let name = unsafe { text(name) };
    record(format!("register {name}"));
    if STATE.with_borrow(|state| state.deny.as_deref() == Some(name.as_str())) {
        // SAFETY: an initialized error from the caller; the static outlives it.
        unsafe { (*error).message = c"denied".as_ptr().cast_mut() };
        return false;
    }
    // SAFETY: writable output from the caller; the sentinel is never dereferenced.
    unsafe { handle.write(NonNull::dangling().as_ptr()) };
    true
}
pub unsafe extern "C" fn category(
    _: *mut LSHandle,
    _: *const c_char,
    methods: *mut LSMethod,
    _: *mut LSSignal,
    _: *mut LSProperty,
    _: *mut LSError,
) -> bool {
    let mut names = Vec::new();
    let mut entry = methods;
    // SAFETY: a sentinel-terminated table from the wrapper.
    unsafe {
        while !(*entry).name.is_null() {
            names.push(text((*entry).name));
            STATE.with_borrow_mut(|state| state.method = (*entry).function);
            entry = entry.add(1);
        }
    }
    record(format!("category {}", names.join(",")));
    true
}
pub unsafe extern "C" fn category_data(
    _: *mut LSHandle,
    _: *const c_char,
    data: *mut c_void,
    _: *mut LSError,
) -> bool {
    record("data");
    STATE.with_borrow_mut(|state| state.data = Some(data));
    true
}
pub unsafe extern "C" fn unregister(_: *mut LSHandle, _: *mut LSError) -> bool {
    record("unregister");
    true
}
pub unsafe extern "C" fn message_ref(_: *mut LSMessage) {}
pub unsafe extern "C" fn application_id(message: *mut LSMessage) -> *const c_char {
    // SAFETY: only fake messages reach the mocks.
    unsafe { fake(message) }.app.as_ptr()
}
pub unsafe extern "C" fn sender_service(_: *mut LSMessage) -> *const c_char {
    c"com.example.service".as_ptr()
}
pub unsafe extern "C" fn method(message: *mut LSMessage) -> *const c_char {
    // SAFETY: only fake messages reach the mocks.
    unsafe { fake(message) }.method.as_ptr()
}
pub unsafe extern "C" fn payload(message: *mut LSMessage) -> *const c_char {
    // SAFETY: only fake messages reach the mocks.
    unsafe { fake(message) }.payload.as_ptr()
}
pub unsafe extern "C" fn reply(
    _: *mut LSHandle,
    _: *mut LSMessage,
    payload: *const c_char,
    _: *mut LSError,
) -> bool {
    // SAFETY: the wrapper passes a NUL-terminated payload.
    record(format!("reply {}", unsafe { text(payload) }));
    true
}
pub unsafe extern "C" fn attach(_: *mut LSHandle, _: *mut GMainLoop, _: *mut LSError) -> bool {
    record("attach");
    true
}
pub unsafe extern "C" fn call(
    _: *mut LSHandle,
    uri: *const c_char,
    _: *const c_char,
    callback: LSFilterFunc,
    data: *mut c_void,
    token: *mut LSMessageToken,
    _: *mut LSError,
) -> bool {
    // SAFETY: the wrapper passes a NUL-terminated URI and a writable token.
    unsafe {
        record(format!("call {}", text(uri)));
        let index = STATE.with_borrow_mut(|state| {
            state.lscalls.push((callback, data));
            state.lscalls.len()
        });
        token.write(LSMessageToken::try_from(index).unwrap());
    }
    true
}
pub unsafe extern "C" fn cancel(_: *mut LSHandle, token: LSMessageToken, _: *mut LSError) -> bool {
    record(format!("cancel {token}"));
    true
}
pub unsafe extern "C" fn subscribe(
    _: *mut LSHandle,
    _: *mut LSMessage,
    subscribed: *mut bool,
    _: *mut LSError,
) -> bool {
    record("subscribe");
    // SAFETY: writable output from the caller.
    unsafe { subscribed.write(true) };
    true
}
pub unsafe extern "C" fn publish(
    _: *mut LSHandle,
    key: *const c_char,
    payload: *const c_char,
    _: *mut LSError,
) -> bool {
    // SAFETY: the wrapper passes NUL-terminated strings.
    record(format!("publish {} {}", unsafe { text(key) }, unsafe {
        text(payload)
    }));
    true
}
pub unsafe extern "C" fn subscribers(_: *mut LSHandle, _: *const c_char) -> c_uint {
    0
}

pub unsafe extern "C" fn context_new() -> *mut GMainContext {
    record("context");
    NonNull::dangling().as_ptr()
}
pub unsafe extern "C" fn unref(_: *mut u8) {
    record("unref");
}
pub unsafe extern "C" fn loop_new(_: *mut GMainContext, _: gboolean) -> *mut GMainLoop {
    NonNull::dangling().as_ptr()
}
pub unsafe extern "C" fn loop_run(_: *mut GMainLoop) {
    record("run");
    if let Some(script) = STATE.with_borrow_mut(|state| state.script.take()) {
        script();
    }
}
pub unsafe extern "C" fn loop_quit(_: *mut GMainLoop) {
    record("quit");
    STATE.with_borrow_mut(|state| state.quit = true);
}
pub unsafe extern "C" fn source_unref(_: *mut GSource) {
    record("source unref");
}
pub unsafe extern "C" fn source_attach(_: *mut GSource, _: *mut GMainContext) -> guint {
    1
}
pub unsafe extern "C" fn source_callback(
    _: *mut GSource,
    function: GSourceFunc,
    data: gpointer,
    _: GDestroyNotify,
) {
    STATE.with_borrow_mut(|state| state.timer = Some((function, data)));
}
pub unsafe extern "C" fn timer_new(interval: guint) -> *mut GSource {
    record(format!("timer {interval}"));
    NonNull::dangling().as_ptr()
}
