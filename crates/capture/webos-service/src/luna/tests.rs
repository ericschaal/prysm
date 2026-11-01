//! In-process ABI stubs exercise Rust ownership and failure paths without a TV.
//! Opaque native handles are non-dereferenced sentinels. Message pointers refer
//! to test-owned `MessageData`; callbacks use real, live Rc storage.
use super::*;
use ffi::{glib as g, luna as l};
use std::os::raw::c_char;

#[derive(Default)]
struct Mock {
    events: Vec<&'static str>,
    fail: Option<&'static str>,
    callback: l::LSFilterFunc,
    data: *mut c_void,
    reenter: bool,
    reply: Option<Value>,
    method: Option<&'static CStr>,
    app_id: Option<&'static CStr>,
    reply_reentry: bool,
}

thread_local! { static MOCK: RefCell<Mock> = RefCell::new(Mock::default()); }

fn event(name: &'static str) -> bool {
    MOCK.with_borrow_mut(|mock| {
        mock.events.push(name);
        mock.fail != Some(name)
    })
}

pub(super) fn process_library() -> libloading::Library {
    libloading::os::unix::Library::this().into()
}

// Miri cannot execute the system loader. These Rust ABI stubs provide only the
// process handle used by the API fixtures; they never resolve or load native code.
#[cfg(miri)]
mod loader {
    use super::{c_char, c_void, ptr};

    // SAFETY: only replaces this unsupported C symbol in the interpreted test binary.
    #[unsafe(export_name = "dlopen")]
    extern "C" fn open(name: *const c_char, _: std::ffi::c_int) -> *mut c_void {
        if name.is_null() {
            Box::into_raw(Box::new(0_u8)).cast()
        } else {
            ptr::null_mut()
        }
    }

    // SAFETY: paired with the test-only dlopen above; absent from production builds.
    #[unsafe(export_name = "dlclose")]
    unsafe extern "C" fn close(handle: *mut c_void) -> std::ffi::c_int {
        // SAFETY: libloading closes each process handle from open() exactly once.
        drop(unsafe { Box::from_raw(handle.cast::<u8>()) });
        0
    }

    // SAFETY: only implements the test loader's static error buffer contract.
    #[unsafe(export_name = "dlerror")]
    extern "C" fn error() -> *mut c_char {
        c"native loading is unavailable in Miri".as_ptr().cast_mut()
    }
}

pub(super) unsafe extern "C" fn error_init(error: *mut l::LSError) -> bool {
    if !event("error_init") {
        return false;
    }
    // SAFETY: check() passes writable, aligned storage of exactly this generated type.
    unsafe {
        error.write(l::LSError {
            error_code: 0,
            message: ptr::null_mut(),
            file: ptr::null(),
            line: 0,
            func: ptr::null(),
            padding: ptr::null_mut(),
            magic: 1,
        });
    }
    true
}

pub(super) unsafe extern "C" fn error_free(error: *mut l::LSError) {
    event("error_free");
    // SAFETY: Error owns this initialized slot; any test error text comes from into_raw.
    let error = unsafe { &mut *error };
    if !error.message.is_null() {
        // SAFETY: take back the one allocation installed by the error-text test.
        drop(unsafe { CString::from_raw(error.message) });
        error.message = ptr::null_mut();
    }
}

pub(super) unsafe extern "C" fn register(
    _: *const c_char,
    handle: *mut *mut LSHandle,
    _: *mut l::LSError,
) -> bool {
    if !event("register") {
        return false;
    }
    // SAFETY: Service::new passes a writable local handle slot. Sentinel is never dereferenced.
    unsafe {
        handle.write(NonNull::dangling().as_ptr());
    }
    true
}

macro_rules! luna_operation {
    ($function:ident, $name:literal, $($argument:ident: $ty:ty),* $(,)?) => {
        pub(super) extern "C" fn $function($($argument: $ty),*) -> bool { event($name) }
    };
}
luna_operation!(category, "category", _h: *mut LSHandle, _c: *const c_char, _m: *mut LSMethod, _s: *mut l::LSSignal, _p: *mut l::LSProperty, _e: *mut l::LSError);
luna_operation!(category_data, "category_data", _h: *mut LSHandle, _c: *const c_char, _d: *mut c_void, _e: *mut l::LSError);
luna_operation!(unregister, "unregister", _h: *mut LSHandle, _e: *mut l::LSError);
luna_operation!(loop_attach, "loop_attach", _h: *mut LSHandle, _l: *mut l::GMainLoop, _e: *mut l::LSError);
luna_operation!(publish, "publish", _h: *mut LSHandle, _k: *const c_char, _v: *const c_char, _e: *mut l::LSError);

pub(super) unsafe extern "C" fn call(
    _: *mut LSHandle,
    _: *const c_char,
    _: *const c_char,
    callback: l::LSFilterFunc,
    data: *mut c_void,
    token: *mut l::LSMessageToken,
    _: *mut l::LSError,
) -> bool {
    if !event("call") {
        return false;
    }
    // SAFETY: Service supplies a live output slot with the SDK-generated token type.
    unsafe {
        token.write(42);
    }
    MOCK.with_borrow_mut(|mock| {
        mock.callback = callback;
        mock.data = data;
    });
    true
}

pub(super) unsafe extern "C" fn cancel(
    handle: *mut LSHandle,
    _: l::LSMessageToken,
    _: *mut l::LSError,
) -> bool {
    event("cancel");
    let (reenter, callback, data) =
        MOCK.with_borrow(|mock| (mock.reenter, mock.callback, mock.data));
    if reenter {
        let mut message = MessageData(Some(CString::new("{\"state\":\"Active\"}").unwrap()));
        // SAFETY: exercise synchronous reentry while Service still owns callback state.
        unsafe {
            callback.unwrap()(handle, ptr::from_mut(&mut message).cast(), data);
        }
    }
    true
}

pub(super) extern "C" fn method(_: *mut LSMessage) -> *const c_char {
    MOCK.with_borrow(|mock| mock.method.unwrap_or(c"status").as_ptr())
}
pub(super) extern "C" fn application_id(_: *mut LSMessage) -> *const c_char {
    MOCK.with_borrow(|mock| mock.app_id.unwrap_or(c"").as_ptr())
}
pub(super) extern "C" fn sender_service(_: *mut LSMessage) -> *const c_char {
    c"".as_ptr()
}
pub(super) extern "C" fn message_ref(_: *mut LSMessage) {
    event("message_ref");
}
pub(super) extern "C" fn message_unref(_: *mut LSMessage) {
    event("message_unref");
}

struct MessageData(Option<CString>);

pub(super) unsafe extern "C" fn payload(message: *mut LSMessage) -> *const c_char {
    // SAFETY: tests construct messages from a live MessageData, never a native LSMessage.
    let message = unsafe { &*message.cast::<MessageData>() };
    message
        .0
        .as_ref()
        .map_or(ptr::null(), |value| value.as_ptr())
}

pub(super) unsafe extern "C" fn reply(
    _: *mut LSHandle,
    _: *mut LSMessage,
    value: *const c_char,
    _: *mut l::LSError,
) -> bool {
    // SAFETY: respond() keeps a valid CString alive through this call.
    let value = unsafe { CStr::from_ptr(value) };
    let value = serde_json::from_slice(value.to_bytes()).unwrap();
    MOCK.with_borrow_mut(|mock| mock.reply = Some(value));
    let reentry = MOCK.with_borrow_mut(|mock| std::mem::take(&mut mock.reply_reentry));
    if reentry {
        let (callback, data) = MOCK.with_borrow(|mock| (mock.callback, mock.data));
        let mut message = MessageData(Some(CString::new("{\"state\":\"Active\"}").unwrap()));
        // SAFETY: simulate a synchronous power notification while the reply is live.
        unsafe {
            callback.unwrap()(
                NonNull::dangling().as_ptr(),
                ptr::from_mut(&mut message).cast(),
                data,
            );
        }
    }
    event("reply")
}

pub(super) unsafe extern "C" fn subscribe(
    _: *mut LSHandle,
    _: *mut LSMessage,
    subscribed: *mut bool,
    _: *mut l::LSError,
) -> bool {
    // SAFETY: Message::subscribe provides a live local bool output.
    unsafe {
        subscribed.write(true);
    }
    event("subscribe")
}
pub(super) extern "C" fn subscribers(_: *mut LSHandle, _: *const c_char) -> u32 {
    0
}

pub(super) extern "C" fn context_new() -> *mut g::GMainContext {
    if event("context_new") {
        NonNull::dangling().as_ptr()
    } else {
        ptr::null_mut()
    }
}
pub(super) extern "C" fn loop_new(_: *mut g::GMainContext, _: i32) -> *mut g::GMainLoop {
    if event("loop_new") {
        NonNull::dangling().as_ptr()
    } else {
        ptr::null_mut()
    }
}
pub(super) extern "C" fn timer_new(_: u32) -> *mut g::GSource {
    if event("timer_new") {
        NonNull::dangling().as_ptr()
    } else {
        ptr::null_mut()
    }
}
pub(super) extern "C" fn source_attach(_: *mut g::GSource, context: *mut g::GMainContext) -> u32 {
    u32::from(!context.is_null() && event("source_attach"))
}
pub(super) extern "C" fn source_callback(
    _: *mut g::GSource,
    _: g::GSourceFunc,
    _: *mut c_void,
    _: g::GDestroyNotify,
) {
    event("source_callback");
}

macro_rules! glib_operation {
    ($function:ident, $ty:ty) => {
        pub(super) extern "C" fn $function(_: *mut $ty) {
            event(stringify!($function));
        }
    };
}
glib_operation!(context_unref, g::GMainContext);
glib_operation!(loop_unref, g::GMainLoop);
glib_operation!(loop_run, g::GMainLoop);
glib_operation!(loop_quit, g::GMainLoop);
glib_operation!(source_unref, g::GSource);
glib_operation!(source_destroy, g::GSource);

fn service() -> Result<Service> {
    Service::new(
        Luna::test_api(),
        Glib::test_api(),
        "test.prysm",
        Some(Duration::from_secs(1)),
        Arc::new(AtomicBool::new(false)),
    )
}

fn reset(fail: Option<&'static str>) {
    MOCK.set(Mock {
        fail,
        ..Mock::default()
    });
}

#[test]
fn invalid_safe_inputs_fail_before_allocating_native_resources() {
    for (name, lifetime) in [
        ("test\0prysm", Duration::ZERO),
        ("test.prysm", Duration::MAX),
    ] {
        reset(None);
        assert!(
            Service::new(
                Luna::test_api(),
                Glib::test_api(),
                name,
                Some(lifetime),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
        );
        assert!(MOCK.with_borrow(|mock| mock.events.is_empty()));
    }
}

#[test]
fn shutdown_flag_and_expiry_quit_the_owned_loop() {
    fn tick(service: &Service) {
        let data = Rc::as_ptr(&service.callbacks).cast_mut().cast();
        // SAFETY: simulate dispatch on the owner thread while its Rc and loop are live.
        let keep_source = unsafe { timer_callback(data) };
        assert_eq!(keep_source, 1);
    }
    reset(None);
    let service = Service::new(
        Luna::test_api(),
        Glib::test_api(),
        "test.prysm",
        Some(Duration::from_secs(300)),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    tick(&service);
    assert!(!MOCK.with_borrow(|mock| mock.events.contains(&"loop_quit")));
    service.callbacks.stop.store(true, Ordering::Relaxed);
    tick(&service);
    assert!(MOCK.with_borrow(|mock| mock.events.contains(&"loop_quit")));
    drop(service);

    reset(None);
    let service = Service::new(
        Luna::test_api(),
        Glib::test_api(),
        "test.prysm",
        Some(Duration::ZERO),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    tick(&service);
    assert!(MOCK.with_borrow(|mock| mock.events.contains(&"loop_quit")));
}

fn cleanup_events() -> Vec<&'static str> {
    MOCK.with_borrow(|mock| {
        mock.events
            .iter()
            .copied()
            .filter(|name| {
                [
                    "source_destroy",
                    "source_unref",
                    "cancel",
                    "unregister",
                    "loop_unref",
                    "context_unref",
                ]
                .contains(name)
            })
            .collect()
    })
}

#[test]
fn cleanup_detaches_callbacks_before_releasing_state_even_with_reentry() {
    reset(None);
    let mut service = service().unwrap();
    service.register().unwrap();
    let weak = Rc::downgrade(&service.callbacks);
    MOCK.with_borrow_mut(|mock| mock.reenter = true);
    drop(service);
    assert!(weak.upgrade().is_none());
    assert_eq!(
        cleanup_events(),
        [
            "source_destroy",
            "source_unref",
            "cancel",
            "unregister",
            "loop_unref",
            "context_unref"
        ]
    );
    assert!(!MOCK.with_borrow(|mock| mock.events.contains(&"publish")));
}

#[test]
fn partial_initialization_releases_only_resources_it_acquired() {
    for failure in [
        "context_new",
        "loop_new",
        "register",
        "category",
        "category_data",
        "loop_attach",
        "timer_new",
        "source_attach",
    ] {
        reset(Some(failure));
        let result = service().and_then(|mut service| service.register());
        assert!(result.is_err(), "{failure}");
        let expected: &[&str] = match failure {
            "context_new" => &[],
            "loop_new" => &["context_unref"],
            "register" => &["loop_unref", "context_unref"],
            "timer_new" => &["cancel", "unregister", "loop_unref", "context_unref"],
            "source_attach" => &[
                "source_destroy",
                "source_unref",
                "cancel",
                "unregister",
                "loop_unref",
                "context_unref",
            ],
            _ => &["unregister", "loop_unref", "context_unref"],
        };
        assert_eq!(cleanup_events(), expected, "{failure}");
    }
}

#[test]
fn error_storage_is_not_read_after_failed_init_and_is_freed_on_unwind() {
    reset(Some("error_init"));
    let luna = Luna::test_api();
    let mut ran = false;
    assert!(
        check(&luna, |_| {
            ran = true;
            true
        })
        .is_err()
    );
    assert!(!ran);
    assert!(!MOCK.with_borrow(|mock| mock.events.contains(&"error_free")));
    reset(None);
    assert!(catch_unwind(AssertUnwindSafe(|| check(&luna, |_| panic!("test unwind")))).is_err());
    assert_eq!(
        MOCK.with_borrow(|mock| mock.events.clone()),
        ["error_init", "error_free"]
    );
}

#[test]
fn native_error_text_is_owned_before_native_storage_is_freed() {
    reset(None);
    let luna = Luna::test_api();
    let error = check(&luna, |error| {
        // SAFETY: check provides its live error slot; the mock free takes ownership.
        unsafe {
            (*error).message = CString::new("native failure").unwrap().into_raw();
            (*error).error_code = -5;
        }
        false
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "native failure (code -5)");
    assert_eq!(
        MOCK.with_borrow(|mock| mock.events.clone()),
        ["error_init", "error_free"]
    );
}

#[test]
fn second_registration_cannot_lose_the_first_callback_sources() {
    reset(None);
    let mut service = service().unwrap();
    service.register().unwrap();
    assert!(service.register().is_err());
    drop(service);
    assert_eq!(
        cleanup_events(),
        [
            "source_destroy",
            "source_unref",
            "cancel",
            "unregister",
            "loop_unref",
            "context_unref"
        ]
    );
}

#[test]
fn callback_messages_reject_null_invalid_and_oversized_payloads() {
    reset(None);
    let service = service().unwrap();
    // SAFETY: null must be rejected without dereferencing it.
    assert!(unsafe { Message::from_callback(&service.callbacks, ptr::null_mut()) }.is_err());
    for payload in [None, Some("{".to_owned()), Some("x".repeat(4097))] {
        let mut raw = MessageData(payload.map(|text| CString::new(text).unwrap()));
        // SAFETY: test stub message is live through the locally borrowed wrapper.
        let message =
            unsafe { Message::from_callback(&service.callbacks, ptr::from_mut(&mut raw).cast()) }
                .unwrap();
        assert!(message.payload().is_err());
        service.callbacks.status(&message).unwrap();
        assert_eq!(
            MOCK.with_borrow(|mock| mock.reply.clone()).unwrap()["returnValue"],
            false
        );
    }
}

#[test]
#[cfg(not(miri))]
fn mutation_authorizes_before_deserializing_configuration() {
    reset(None);
    let dir = crate::application::tests::sandbox();
    let service = service().unwrap();
    *service.callbacks.app.borrow_mut() = Some(
        crate::application::App::new(crate::files::Files::sandbox(dir.path()).unwrap()).unwrap(),
    );
    let mut raw = MessageData(Some(CString::new("{}").unwrap()));
    // SAFETY: the test-owned message lives through all callback invocations.
    let message =
        unsafe { Message::from_callback(&service.callbacks, ptr::from_mut(&mut raw).cast()) }
            .unwrap();
    MOCK.with_borrow_mut(|mock| {
        mock.method = Some(c"setConfig");
        mock.app_id = Some(c"untrusted");
    });
    service.callbacks.status(&message).unwrap();
    let denied = MOCK.with_borrow(|mock| mock.reply.clone()).unwrap();
    assert_eq!(denied["returnValue"], false);
    assert_eq!(
        denied["errorText"],
        "Only the Prysm Capture app may change service state"
    );
    MOCK.with_borrow_mut(|mock| mock.app_id = Some(c"io.prysm.capture.probe.app session"));
    service.callbacks.status(&message).unwrap();
    let invalid = MOCK.with_borrow(|mock| mock.reply.clone()).unwrap();
    assert_eq!(invalid["returnValue"], false);
    assert!(
        invalid["errorText"]
            .as_str()
            .unwrap()
            .contains("missing field")
    );
    assert!(!dir.path().join("config.json").exists());
    raw.0 = Some(CString::new(r#"{"receiver":"127.0.0.1:19400","source":"video","compression":"none","autostart":false}"#).unwrap());
    service.callbacks.status(&message).unwrap();
    let saved = MOCK.with_borrow(|mock| mock.reply.clone()).unwrap();
    assert_eq!(saved["returnValue"], true);
    let persisted: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("config.json")).unwrap()).unwrap();
    assert_eq!(saved["config"], persisted);
}

#[test]
#[cfg(not(miri))]
fn fatal_boundary_failures_abort_without_unwinding() {
    use std::os::unix::process::ExitStatusExt;
    if let Ok(mode) = std::env::var("PRYSM_TEST_BOUNDARY_FAILURE") {
        struct PanickingPayload;
        impl Drop for PanickingPayload {
            fn drop(&mut self) {
                panic!("panic payload destructor must not run");
            }
        }
        if mode == "unregister" {
            reset(Some("unregister"));
            drop(service().unwrap());
            panic!("state released after failed unregistration");
        }
        callback(|| std::panic::panic_any(PanickingPayload));
        panic!("callback resumed after a panic");
    }
    for mode in ["callback", "unregister"] {
        let status = crate::application::tests::test_process()
            .args([
                "--exact",
                "luna::tests::fatal_boundary_failures_abort_without_unwinding",
            ])
            .env("PRYSM_TEST_BOUNDARY_FAILURE", mode)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.signal(), Some(6), "{mode}");
    }
}

#[test]
fn retained_messages_are_released_before_unregister_even_with_cancel_reentry() {
    reset(None);
    let mut service = service().unwrap();
    service.register().unwrap();
    let mut raw = MessageData(Some(CString::new("{}").unwrap()));
    // SAFETY: test-owned message remains live until after the retained reference is released.
    let message =
        unsafe { Message::from_callback(&service.callbacks, ptr::from_mut(&mut raw).cast()) }
            .unwrap();
    *service.callbacks.pending.borrow_mut() = Some(message.retain());
    MOCK.with_borrow_mut(|mock| mock.reenter = true);
    drop(service);
    let events = MOCK.with_borrow(|mock| mock.events.clone());
    assert_eq!(events.iter().filter(|e| **e == "message_ref").count(), 1);
    assert_eq!(events.iter().filter(|e| **e == "message_unref").count(), 1);
    assert!(
        events.iter().position(|e| *e == "message_unref")
            < events.iter().position(|e| *e == "unregister")
    );
}

#[test]
#[cfg(not(miri))]
fn deferred_reply_survives_publish_failure_and_releases_reference_on_reply_failure() {
    use crate::{
        application::{
            App,
            tests::{sandbox, script},
        },
        files::Files,
    };
    for (fail, configured) in [("publish", false), ("publish", true), ("reply", true)] {
        reset(None);
        let dir = sandbox();
        script(
            &dir.path().join("scan-services"),
            "while [ ! -f \"$0.ready\" ]; do sleep 0.01; done",
        );
        let mut service = service().unwrap();
        service.register().unwrap();
        let files = Files::sandbox(dir.path()).unwrap();
        if configured {
            files
                .persist(&crate::config::Config {
                    receiver: Some("127.0.0.1:42".parse().unwrap()),
                    ..crate::config::Config::default()
                })
                .unwrap();
        }
        *service.callbacks.app.borrow_mut() = Some(App::new(files).unwrap());
        MOCK.with_borrow_mut(|mock| {
            mock.method = Some(c"start");
            mock.app_id = Some(c"io.prysm.capture.probe.app session");
            mock.fail = Some(fail);
        });
        let mut raw = MessageData(Some(CString::new("{}").unwrap()));
        // SAFETY: the fixture stays alive through completion/unref, just as LS2 retain promises.
        let message =
            unsafe { Message::from_callback(&service.callbacks, ptr::from_mut(&mut raw).cast()) }
                .unwrap();
        let _ = service.callbacks.status(&message);
        assert!(service.callbacks.pending.borrow().is_some());
        std::fs::write(dir.path().join("scan-services.ready"), "").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        MOCK.with_borrow_mut(|mock| mock.reply_reentry = true);
        while service.callbacks.pending.borrow().is_some() {
            let _ = service.callbacks.tick(Instant::now());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            MOCK.with_borrow(|mock| mock.reply.clone()).unwrap()["returnValue"],
            configured
        );
        drop(service);
        assert_eq!(
            MOCK.with_borrow(|mock| mock
                .events
                .iter()
                .filter(|e| **e == "message_unref")
                .count()),
            1
        );
    }
}

#[test]
fn initial_power_subscription_failure_keeps_service_available_and_retries() {
    reset(Some("call"));
    let mut service = service().unwrap();
    service.register().unwrap();
    assert!(service.callbacks.snapshot(false)["power"].is_null());
    assert!(service.callbacks.power_retry.get().is_some());
    MOCK.with_borrow_mut(|mock| mock.fail = None);
    service
        .callbacks
        .tick(Instant::now() + Duration::from_secs(5))
        .unwrap();
    assert_eq!(service.callbacks.power_token.get(), 42);
    assert!(service.callbacks.power_retry.get().is_none());
}

#[test]
#[cfg(not(miri))]
fn initial_power_failure_remains_visible_when_supervisor_status_is_merged() {
    reset(Some("call"));
    let dir = crate::application::tests::sandbox();
    let mut service = service().unwrap();
    *service.callbacks.app.borrow_mut() = Some(
        crate::application::App::new(crate::files::Files::sandbox(dir.path()).unwrap()).unwrap(),
    );
    service.register().unwrap();
    let value = service.callbacks.snapshot(false);
    assert!(value["power"].is_null());
    assert!(value["error"].is_string());
    assert_eq!(value["senderRunning"], false);
}

#[test]
fn status_subscription_reply_acknowledges_the_retained_native_subscription() {
    reset(None);
    let service = service().unwrap();
    let mut raw = MessageData(Some(CString::new(r#"{"subscribe":true}"#).unwrap()));
    // SAFETY: the test message remains alive for this entire callback invocation.
    let message =
        unsafe { Message::from_callback(&service.callbacks, ptr::from_mut(&mut raw).cast()) }
            .unwrap();
    service.callbacks.status(&message).unwrap();
    assert_eq!(
        MOCK.with_borrow(|mock| mock.reply.clone()).unwrap()["subscribed"],
        true
    );
}

#[test]
fn readiness_uses_main_geometry_for_every_hdmi_input_and_video_apps() {
    for source in ["HDMI", "VDEC"] {
        for port in 0..4 {
            let mut value = json!({"returnValue":true,"video":[
                {"sink":"SUB0","connected":true,"width":100,"height":100,"frameRate":60},
                {"sink":"MAIN","connected":true,"connectedSource":source,
                 "connectedSourcePort":port,"width":3840,"height":2160,"frameRate":50,"muted":true}
            ]});
            assert_eq!(
                VideoObservation::parse(&value).availability,
                VideoAvailability::Available
            );
            value["video"][1]["width"] = json!(0);
            assert_eq!(
                VideoObservation::parse(&value).availability,
                VideoAvailability::Unavailable
            );
            value["video"][1] = json!({"sink":"MAIN","connected":false});
            assert_eq!(
                VideoObservation::parse(&value).availability,
                VideoAvailability::Unavailable
            );
        }
    }
    for value in [
        Value::Null,
        json!({"returnValue":false}),
        json!({"returnValue":true,"video":[]}),
        json!({"returnValue":true,"video":[{"sink":"MAIN","connected":true}]}),
    ] {
        assert_eq!(
            VideoObservation::parse(&value).availability,
            VideoAvailability::Unknown
        );
    }
}

#[test]
fn video_subscription_failure_retries_and_drop_cancels_both_owned_calls() {
    reset(None);
    let mut service = Service::new(
        Luna::test_api(),
        Glib::test_api(),
        "test.prysm",
        None,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    service.register().unwrap();
    let data = Rc::as_ptr(&service.callbacks).cast_mut().cast();
    MOCK.with_borrow_mut(|mock| mock.fail = Some("call"));
    assert!(service.callbacks.subscribe_video(data).is_err());
    assert!(service.callbacks.video_retry.get().is_some());
    MOCK.with_borrow_mut(|mock| mock.fail = None);
    service
        .callbacks
        .tick(Instant::now() + Duration::from_secs(6))
        .unwrap();
    assert_ne!(service.callbacks.video_token.get(), 0);
    MOCK.with_borrow_mut(|mock| {
        mock.events.clear();
        mock.reenter = true;
    });
    drop(service);
    assert_eq!(
        MOCK.with_borrow(|mock| mock.events.iter().filter(|e| **e == "cancel").count()),
        2
    );
}

#[test]
#[cfg(not(miri))]
fn video_callback_updates_context_and_recovers_after_subscription_failure() {
    use crate::{application::App, files::Files};
    reset(Some("call"));
    let dir = crate::application::tests::sandbox();
    let mut service = Service::new(
        Luna::test_api(),
        Glib::test_api(),
        "test.prysm",
        None,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let files = Files::sandbox(dir.path()).unwrap();
    files.persist(&serde_json::from_value(json!({
        "receiver":"127.0.0.1:19400", "source":"video", "compression":"none", "autostart":false
    })).unwrap()).unwrap();
    crate::application::tests::script(&files.worker, r#"exec cat > "$0.control""#);
    *service.callbacks.app.borrow_mut() = Some(App::new(files).unwrap());
    service.register().unwrap();
    assert!(service.callbacks.video_retry.get().is_some());
    {
        let mut app = service.callbacks.app.borrow_mut();
        let app = app.as_mut().unwrap();
        app.state.start(Instant::now()).unwrap();
        app.power(Some("Active".into()), None, Instant::now());
        app.tick(Instant::now());
    }
    let data = Rc::as_ptr(&service.callbacks).cast_mut().cast();
    for (source, port, context, commands) in [
        ("HDMI", 0, "a", "1"),
        ("HDMI", 0, "a", "1"),
        ("HDMI", 3, "a", "1r"),
        ("HDMI", 3, "b", "1rr"),
        ("VDEC", 3, "b", "1rrr"),
    ] {
        let value = json!({"returnValue":true,"subscribed":true,"video":[{
            "sink":"MAIN","connected":true,"width":1920,"height":1080,"frameRate":60,
            "connectedSource":source,"connectedSourcePort":port,"context":context
        }]});
        let mut raw = MessageData(Some(CString::new(value.to_string()).unwrap()));
        // SAFETY: callback borrows live test message and retained Service state on this thread.
        assert!(unsafe {
            video_callback(
                service.callbacks.handle.as_ptr(),
                ptr::from_mut(&mut raw).cast(),
                data,
            )
        });
        assert!(service.callbacks.video_retry.get().is_none());
        let expected = VideoContext {
            source: json!(source),
            port: json!(port),
            context: json!(context),
        };
        assert_eq!(
            service.callbacks.video_context.borrow().as_ref(),
            Some(&expected)
        );
        service
            .callbacks
            .app
            .borrow_mut()
            .as_mut()
            .unwrap()
            .tick(Instant::now());
        let deadline = Instant::now() + Duration::from_secs(2);
        while std::fs::read_to_string(dir.path().join("worker.control")).unwrap_or_default()
            != commands
        {
            assert!(
                Instant::now() < deadline,
                "worker did not receive {commands:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
