//! The wrapper against recording stand-ins for LS2 and GLib.
use super::*;
use crate::mock::{self, FakeMessage};
use std::collections::VecDeque;
use std::time::Duration;

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
    /// Due again in 100 ms, five polls in all.
    fn poll(&mut self, _: &mut Bus, now: Instant) -> Next {
        self.ticks += 1;
        if self.ticks < 5 {
            Next::At(now + Duration::from_millis(100))
        } else {
            Next::Stop
        }
    }
}

fn config(wake: &Wake) -> Config<'_> {
    Config {
        name: "io.github.ericschaal.prysm.service",
        methods: &["getState", "uninstall"],
        calls: &[("luna://power/get", "{}"), ("luna://video/get", "{}")],
        wake,
        refuse,
    }
}

fn run_mock(handler: impl Handler + 'static) -> Result<(), Error> {
    let wake = Wake::new().unwrap();
    run_with(
        mock::luna(),
        mock::glib(),
        &config(&wake),
        Box::new(handler),
    )
}

/// The test service's error reply: the refusal and its text (no quotes in them).
fn refuse(refusal: Refusal, text: &str) -> String {
    format!(r#"{{"returnValue":false,"refusal":"{refusal:?}","errorText":"{text}"}}"#)
}

fn index(calls: &[String], call: &str) -> usize {
    calls
        .iter()
        .position(|c| c == call)
        .unwrap_or_else(|| panic!("no `{call}` in {calls:#?}"))
}

#[test]
fn requests_replies_and_polls_reach_the_handler_and_teardown_is_ordered() {
    mock::reset();
    mock::on_run(|| {
        mock::invoke_method(&mut FakeMessage::new(
            "getState",
            "io.github.ericschaal.prysm 1234",
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
    run_mock(recorder).unwrap();

    assert_eq!(
        *seen.borrow(),
        [
            r#"request getState from io.github.ericschaal.prysm: {"subscribe":true}"#,
            r#"reply 0: {"state":"Active"}"#,
            "reply 1: {}",
        ]
    );
    let calls = mock::calls();
    assert!(calls.contains(&"category getState,uninstall".to_owned()));
    assert!(
        calls.contains(&"watch 1".to_owned()),
        "the wake socket is watched"
    );
    // The channel never closes the socket, and only the watch keeps it.
    let channel = [
        "channel",
        "channel close on unref 0",
        "watch 1",
        "channel unref",
    ]
    .map(|call| index(&calls, call));
    assert!(channel.is_sorted(), "{calls:#?}");
    // The first poll is due at once, inside the loop.
    assert!(index(&calls, "timer 0") < index(&calls, "run"));
    // Subscribe, then reply, then the deferred publish.
    let subscribed = index(&calls, "subscribe");
    let replied = index(&calls, r#"reply {"returnValue":true}"#);
    let published = index(&calls, r#"publish /getState {"n":1}"#);
    assert!(subscribed < replied && replied < published);
    // The oversized payload got an error reply without reaching the handler.
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("reply {\"returnValue\":false")
                && c.contains(r#""refusal":"TooLarge""#)
                && c.contains("exceeds"))
    );
    // The recall cancelled call 2 and made it again.
    let cancelled = index(&calls, "cancel 2");
    assert_eq!(calls[cancelled + 3], "call luna://video/get");
    // Teardown: the watch, both calls, unregister, then the GLib references.
    let order = [
        "quit",
        "watch destroy",
        "cancel 1",
        "cancel 3",
        "unregister",
    ]
    .map(|c| index(&calls, c));
    assert!(order.is_sorted(), "{calls:#?}");
    let unregistered = order[4];
    assert_eq!(&calls[unregistered + 2..], ["unref", "unref"]);
}

#[test]
fn a_refused_registration_releases_the_loop_and_context() {
    mock::reset();
    mock::deny_register(Some("io.github.ericschaal.prysm.service"));
    let error = run_mock(Recorder::default()).unwrap_err();
    assert!(error.to_string().contains("Invalid permissions"), "{error}");
    let calls = mock::calls();
    assert!(!calls.contains(&"unregister".to_owned()));
    assert!(!calls.contains(&"run".to_owned()));
    assert_eq!(&calls[calls.len() - 2..], ["unref", "unref"]);
}

/// Run the service once, delivering one `getState` request (the recorder
/// subscribes the caller and publishes), with LS call `failing` failing.
fn one_request(failing: &'static str) -> Vec<String> {
    mock::reset();
    mock::fail(Some(failing));
    mock::on_run(|| {
        mock::invoke_method(&mut FakeMessage::new("getState", "app", "{}"));
        while !mock::invoke_timer() {}
    });
    run_mock(Recorder::default()).unwrap();
    mock::calls()
}

#[test]
fn a_failed_subscribe_or_reply_still_answers_and_publishes() {
    for failing in ["subscribe", "reply"] {
        let calls = one_request(failing);
        let error_reply = calls
            .iter()
            .position(|c| c.starts_with(r#"reply {"returnValue":false"#))
            .unwrap_or_else(|| panic!("{failing}: the caller got no answer: {calls:#?}"));
        assert!(
            calls[error_reply].contains("failed")
                && calls[error_reply].contains(r#""refusal":"Internal""#),
            "{}",
            calls[error_reply]
        );
        index(&calls, r#"publish /getState {"n":1}"#);
    }
}

#[test]
fn a_failure_after_registering_still_unregisters() {
    for failing in ["category", "data", "attach"] {
        mock::reset();
        mock::fail(Some(failing));
        let error = run_mock(Recorder::default()).unwrap_err();
        assert!(error.to_string().contains("failed"), "{failing}: {error}");
        let calls = mock::calls();
        assert!(!calls.contains(&"run".to_owned()), "{failing}");
        let unregistered = index(&calls, "unregister");
        assert_eq!(&calls[unregistered + 2..], ["unref", "unref"], "{failing}");
    }
}

/// A handler whose request makes another request of the same service.
struct Reentrant;

impl Handler for Reentrant {
    fn request(&mut self, _: &mut Bus, _: Request<'_>) -> Response {
        mock::invoke_method(&mut FakeMessage::new("uninstall", "app", "{}"));
        Response {
            payload: r#"{"returnValue":true}"#.into(),
            subscribe: false,
        }
    }
    fn reply(&mut self, _: &mut Bus, _: usize, _: &str) {}
    fn poll(&mut self, _: &mut Bus, _: Instant) -> Next {
        Next::Stop
    }
}

#[test]
fn a_request_while_the_handler_runs_is_answered_busy() {
    mock::reset();
    mock::on_run(|| {
        mock::invoke_method(&mut FakeMessage::new("getState", "app", "{}"));
    });
    run_mock(Reentrant).unwrap();
    let calls = mock::calls();
    let busy = calls
        .iter()
        .position(|c| c.contains("the service is busy"))
        .unwrap_or_else(|| panic!("no busy reply: {calls:#?}"));
    assert!(busy < index(&calls, r#"reply {"returnValue":true}"#));
}

#[test]
fn a_reply_while_closing_does_not_reach_the_handler() {
    mock::reset();
    mock::on_run(|| while !mock::invoke_timer() {});
    mock::on_cancel(|| mock::invoke_call(0, r#"{"late":true}"#));
    let recorder = Recorder::default();
    let seen = recorder.seen.clone();
    run_mock(recorder).unwrap();
    assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());
    index(&mock::calls(), "unregister");
}

/// A handler that answers its polls from a script and counts them; its
/// first poll renews call 0.
struct Scripted {
    next: VecDeque<Next>,
    polls: Rc<Cell<usize>>,
}

impl Handler for Scripted {
    fn request(&mut self, _: &mut Bus, _: Request<'_>) -> Response {
        Response {
            payload: r#"{"returnValue":true}"#.into(),
            subscribe: false,
        }
    }
    fn reply(&mut self, _: &mut Bus, _: usize, _: &str) {}
    fn poll(&mut self, bus: &mut Bus, _: Instant) -> Next {
        self.polls.set(self.polls.get() + 1);
        if self.polls.get() == 1 {
            bus.recall(0);
        }
        self.next.pop_front().unwrap_or(Next::Stop)
    }
}

/// The handler runs at the time it asks for, at a wake, and at the loop's
/// next turn after a reply or a request, also one LS2 delivers while a poll
/// runs; with nothing due, no timer is armed.
#[test]
fn the_handler_polls_when_due_woken_or_called() {
    mock::reset();
    let polls = Rc::new(Cell::new(0));
    let handler = Scripted {
        next: VecDeque::from([
            Next::Event,
            Next::Event,
            Next::At(Instant::now() + Duration::from_secs(60)),
            Next::Event,
            Next::Stop,
        ]),
        polls: polls.clone(),
    };
    let wake = Wake::new().unwrap();
    let waker = wake.waker();
    let counted = polls.clone();
    // The first poll's renewal brings a reply while that poll runs.
    mock::on_cancel(|| mock::invoke_call(0, "{}"));
    mock::on_run(move || {
        assert!(!mock::invoke_timer(), "the first poll");
        assert_eq!(counted.get(), 1);
        assert!(mock::armed(), "the reply during it is polled for next");
        assert!(!mock::invoke_timer());
        assert_eq!(counted.get(), 2);
        assert!(!mock::armed(), "nothing due: no timer");
        waker.wake();
        waker.wake();
        mock::invoke_wake();
        assert_eq!(counted.get(), 3, "two wakes, one poll");
        assert!(mock::armed(), "due in a minute");
        mock::invoke_call(0, "{}");
        assert_eq!(counted.get(), 3, "a reply is polled for at the next turn");
        assert!(!mock::invoke_timer());
        assert_eq!(counted.get(), 4);
        assert!(!mock::armed());
        // A request is answered before the poll that may end the loop.
        mock::invoke_method(&mut FakeMessage::new("getState", "app", "{}"));
        assert_eq!(counted.get(), 4);
        assert!(mock::invoke_timer(), "stopped");
    });
    run_with(
        mock::luna(),
        mock::glib(),
        &config(&wake),
        Box::new(handler),
    )
    .unwrap();
    assert_eq!(polls.get(), 5);
    let calls = mock::calls();
    // Rounded up to the millisecond, from a minute after the script began.
    let minute = |call: &String| {
        call.strip_prefix("timer ")
            .and_then(|ms| ms.parse::<u32>().ok())
            .is_some_and(|ms| (59_000..=60_000).contains(&ms))
    };
    assert!(calls.iter().any(minute), "{calls:#?}");
    index(&calls, "quit");
}

#[cfg(not(target_os = "linux"))]
#[test]
fn run_refuses_off_webos() {
    let wake = Wake::new().unwrap();
    let error = run(&config(&wake), Box::new(Recorder::default())).unwrap_err();
    assert!(matches!(error, Error::Unsupported(_)));
}
