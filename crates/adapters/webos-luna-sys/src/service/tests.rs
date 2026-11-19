//! The wrapper against recording stand-ins for LS2 and GLib.
use super::*;
use crate::mock::{self, FakeMessage};

#[derive(Default)]
struct Recorder {
    seen: Rc<RefCell<Vec<String>>>,
    ticks: usize,
}

impl Handler for Recorder {
    fn request(&mut self, bus: &mut Bus, request: Request<'_>) -> Response {
        self.seen.borrow_mut().push(format!(
            "request {} from {}: {}",
            request.method, request.sender, request.payload
        ));
        bus.publish("/getState", r#"{"n":1}"#);
        Response {
            payload: r#"{"returnValue":true}"#.into(),
            subscribe: request.method == "getState",
        }
    }
    fn reply(&mut self, bus: &mut Bus, index: usize, payload: &str) {
        self.seen
            .borrow_mut()
            .push(format!("reply {index}: {payload}"));
        if index == 1 {
            bus.recall(1);
        }
    }
    fn tick(&mut self, _: &mut Bus, _: Instant) -> bool {
        self.ticks += 1;
        self.ticks < 3
    }
}

const CONFIG: Config<'static> = Config {
    name: "io.prysm.tv.service",
    methods: &["getState", "uninstall"],
    calls: &[("luna://power/get", "{}"), ("luna://video/get", "{}")],
    tick: Duration::from_millis(100),
};

fn index(calls: &[String], call: &str) -> usize {
    calls
        .iter()
        .position(|c| c == call)
        .unwrap_or_else(|| panic!("no `{call}` in {calls:#?}"))
}

#[test]
fn requests_replies_and_ticks_reach_the_handler_and_teardown_is_ordered() {
    mock::reset();
    mock::on_run(|| {
        mock::invoke_method(&mut FakeMessage::new(
            "getState",
            "io.prysm.tv 1234",
            r#"{"subscribe":true}"#,
        ));
        let oversized = format!(r#"{{"x":"{}"}}"#, "y".repeat(MAX_PAYLOAD));
        mock::invoke_method(&mut FakeMessage::new("uninstall", "", &oversized));
        mock::invoke_call(0, r#"{"state":"Active"}"#);
        mock::invoke_call(1, "{}");
        let mut fired = 0;
        while !mock::invoke_timer() {
            fired += 1;
            assert!(fired < 10, "the handler asked to stop");
        }
    });
    let recorder = Recorder::default();
    let seen = recorder.seen.clone();
    run_with(mock::luna(), mock::glib(), &CONFIG, Box::new(recorder)).unwrap();

    assert_eq!(
        *seen.borrow(),
        [
            r#"request getState from io.prysm.tv: {"subscribe":true}"#,
            r#"reply 0: {"state":"Active"}"#,
            "reply 1: {}",
        ]
    );
    let calls = mock::calls();
    assert!(calls.contains(&"category getState,uninstall".to_owned()));
    assert!(calls.contains(&"timer 100".to_owned()));
    // Subscribe, then reply, then the deferred publish.
    let subscribed = index(&calls, "subscribe");
    let replied = index(&calls, r#"reply {"returnValue":true}"#);
    let published = index(&calls, r#"publish /getState {"n":1}"#);
    assert!(subscribed < replied && replied < published);
    // The oversized payload got an error reply without reaching the handler.
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("reply {\"returnValue\":false") && c.contains("exceeds"))
    );
    // The recall cancelled call 2 and made it again.
    let cancelled = index(&calls, "cancel 2");
    assert_eq!(calls[cancelled + 3], "call luna://video/get");
    // Teardown: timer, both calls, unregister, then the GLib references.
    let order =
        ["quit", "source unref", "cancel 1", "cancel 3", "unregister"].map(|c| index(&calls, c));
    assert!(order.is_sorted(), "{calls:#?}");
    let unregistered = order[4];
    assert_eq!(&calls[unregistered + 2..], ["unref", "unref"]);
}

#[test]
fn a_refused_registration_releases_the_loop_and_context() {
    mock::reset();
    mock::deny_register(Some("io.prysm.tv.service"));
    let error = run_with(
        mock::luna(),
        mock::glib(),
        &CONFIG,
        Box::new(Recorder::default()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("denied"), "{error}");
    let calls = mock::calls();
    assert!(!calls.contains(&"unregister".to_owned()));
    assert!(!calls.contains(&"run".to_owned()));
    assert_eq!(&calls[calls.len() - 2..], ["unref", "unref"]);
}

#[test]
fn error_replies_are_valid_json() {
    let reply = error_reply("bad \"quote\" \\ and\nnewline");
    assert_eq!(
        reply,
        r#"{"returnValue":false,"errorText":"bad \"quote\" \\ and\u000anewline"}"#
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
fn run_refuses_off_webos() {
    let error = run(&CONFIG, Box::new(Recorder::default())).unwrap_err();
    assert!(matches!(error, Error::Unsupported(_)));
}
