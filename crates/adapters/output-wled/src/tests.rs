//! Loopback fixtures stand in for the device: a scripted HTTP listener and a UDP socket.
use crate::address::{DeviceAddress, Host};
use crate::ddp::{Gamma, Transfer};
use crate::http::{self, RequestError, TIMEOUT};
use crate::observe::{BACKOFF, EVERY, Observer, REPLY};
use crate::output::NotTurnedOff;
use crate::{IDLE_LIMIT, LedCounts, MAX_LEDS, OPEN_LIMIT, Wled, WledSettings};
use prysm_core::{ColorStrip, EdgeColors, LinearColor};
use prysm_ports::{
    AdapterSettings, DeviceBrightness, Error, Hangup, IdleMode, LedOutput, OpenFailure,
    OutputFactory, OutputIssue, Registry,
};
use prysm_testkit::eventually;
use serde_json::{Value, json};
use std::error::Error as _;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(5);
const OK: &str = r#"{"success":true}"#;

#[derive(Clone)]
enum Reply {
    /// 200 with this JSON body and a Content-Length.
    Json(&'static str),
    /// These exact bytes, then close.
    Raw(String),
    /// No reply: hold the connection until the client closes it.
    Hang,
}

struct Device {
    wled: Wled,
    http_port: u16,
    udp: UdpSocket,
    calls: mpsc::Receiver<(String, Value)>,
}

impl Device {
    /// Answers one connection per reply, in order.
    fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_read_timeout(Some(WAIT)).unwrap();
        let http_port = listener.local_addr().unwrap().port();
        let wled = Wled::local(udp.local_addr().unwrap().port());
        let (sender, calls) = mpsc::channel();
        thread::spawn(move || {
            for reply in replies {
                let mut socket = eventually("a connection", WAIT, || match listener.accept() {
                    Ok((socket, _)) => Some(socket),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => None,
                    Err(error) => panic!("{error}"),
                });
                socket.set_nonblocking(false).unwrap();
                socket.set_read_timeout(Some(WAIT)).unwrap();
                let (head, json) = read_request(&mut socket);
                if sender.send((head, json)).is_err() {
                    return;
                }
                let response = match reply {
                    Reply::Json(body) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\n\r\n{body}",
                        body.len()
                    ),
                    Reply::Raw(bytes) => bytes,
                    Reply::Hang => {
                        let _ = socket.read(&mut [0; 1]);
                        continue;
                    }
                };
                let _ = socket.write_all(response.as_bytes());
            }
        });
        Self {
            wled,
            http_port,
            udp,
            calls,
        }
    }

    /// An observer that only checks the state.
    fn observer(&self) -> Observer {
        self.observer_with_config().state_only()
    }

    /// The HTTP fixture's address.
    fn http(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.http_port))
    }

    /// An observer that also reads the configuration, as an open output's.
    fn observer_with_config(&self) -> Observer {
        Observer::new(
            self.http(),
            &self.address("127.0.0.1"),
            Duration::ZERO,
            Instant::now(),
        )
    }

    /// The next request must be a state check.
    fn check_request(&self) {
        self.get_request("/json/si");
    }

    /// Run `poll` until a state check reached the device: a check connects
    /// and sends across polls.
    fn check_sent(&self, mut poll: impl FnMut()) {
        let (head, _) = eventually("the check to be sent", WAIT, || {
            poll();
            self.calls.try_recv().ok()
        });
        assert!(head.starts_with("GET /json/si HTTP/1.1\r\n"), "{head}");
    }

    /// The next request must be `GET path`.
    fn get_request(&self, path: &str) {
        let (head, _) = self.calls.recv_timeout(WAIT).expect("no check");
        assert!(
            head.starts_with(&format!("GET {path} HTTP/1.1\r\n")),
            "{head}"
        );
        let host = format!("\r\nHost: {}\r\n", self.address("127.0.0.1"));
        assert!(head.contains(&host), "{head}");
    }

    /// `host:port` of the HTTP fixture.
    fn address(&self, host: &str) -> String {
        format!("{host}:{}", self.http_port)
    }

    fn call(&self) -> Value {
        self.call_with_head().1
    }

    fn call_with_head(&self) -> (String, Value) {
        let (head, json) = self.calls.recv_timeout(WAIT).expect("no HTTP request");
        assert!(head.starts_with("POST /json/state HTTP/1.1\r\n"), "{head}");
        (head, json)
    }

    fn expect(&self, states: &[Value]) {
        for state in states {
            assert_eq!(&self.call(), state);
        }
        assert!(
            self.calls.recv_timeout(Duration::from_millis(50)).is_err(),
            "unexpected request"
        );
    }

    fn open(&self, leds: LedCounts) -> Result<Box<dyn LedOutput>, Error> {
        self.open_with(leds, true)
    }

    fn open_with(&self, leds: LedCounts, dither: bool) -> Result<Box<dyn LedOutput>, Error> {
        self.wled
            .open(&self.settings(leds, dither), &Hangup::default())
            .map_err(OpenFailure::into_error)
    }

    fn settings(&self, leds: LedCounts, dither: bool) -> WledSettings {
        WledSettings {
            address: self.address("127.0.0.1"),
            leds,
            dither,
        }
    }

    /// The RGB payload of one single-packet DDP frame.
    fn rgb(&self) -> Vec<u8> {
        let mut bytes = [0; 1500];
        let count = self.udp.recv(&mut bytes).expect("no DDP packet");
        assert_eq!(bytes[0], 0x41, "DDP v1 with push");
        assert_eq!(&bytes[2..4], &[0x0b, 1], "RGB8 to the default output");
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0], "offset");
        assert_eq!(
            usize::from(u16::from_be_bytes([bytes[8], bytes[9]])),
            count - 10
        );
        bytes[10..count].to_vec()
    }
}

fn read_request(socket: &mut impl Read) -> (String, Value) {
    let mut bytes = Vec::new();
    let mut chunk = [0; 1024];
    let end = loop {
        let count = socket.read(&mut chunk).unwrap();
        assert_ne!(count, 0, "request ended early");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let Some(length) = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().unwrap())
    }) else {
        return (head, Value::Null);
    };
    while bytes.len() < end + length {
        let count = socket.read(&mut chunk).unwrap();
        assert_ne!(count, 0, "body ended early");
        bytes.extend_from_slice(&chunk[..count]);
    }
    (
        head,
        serde_json::from_slice(&bytes[end..end + length]).unwrap(),
    )
}

fn release() -> Value {
    json!({"live": false})
}
fn live() -> Value {
    json!({"on": true, "lor": 0, "live": true, "tt": 0})
}
fn off() -> Value {
    json!({"on": false, "lor": 2, "tt": 0})
}
fn one_each() -> LedCounts {
    LedCounts {
        top: 1,
        right: 1,
        bottom: 1,
        left: 1,
    }
}
fn fill(r: f32, g: f32, b: f32) -> ColorStrip {
    ColorStrip::fill(LinearColor::new(r, g, b), NonZeroUsize::MIN)
}

fn source(error: &Error) -> &(dyn std::error::Error + Send + Sync + 'static) {
    let Error::Adapter {
        source: Some(source),
        ..
    } = error
    else {
        panic!("expected an adapter error with a source: {error:?}");
    };
    source.as_ref()
}

fn request_error(error: &Error) -> &RequestError {
    source(error).downcast_ref().expect("HTTP request error")
}

#[test]
fn open_through_the_registry_releases_then_enters_live_mode() {
    let device = Device::new(vec![Reply::Json(OK), Reply::Json(OK)]);
    let mut registry = Registry::default();
    registry.add_output(Wled::local(device.udp.local_addr().unwrap().port()));
    let settings = AdapterSettings::new(
        Wled::KIND,
        &WledSettings {
            address: device.address("127.0.0.1"),
            leds: one_each(),
            dither: true,
        },
    );
    let _output = registry.open_output(&settings, &Hangup::default()).unwrap();
    device.expect(&[release(), live()]);
}

#[test]
fn write_sends_ddp_clockwise_from_bottom_left() {
    let device = Device::new(vec![Reply::Json(OK); 2]);
    let mut output = device.open(one_each()).unwrap();
    device.expect(&[release(), live()]);
    let edges = EdgeColors::new(
        fill(1.0, 0.0, 0.0),
        fill(0.0, 1.0, 0.0),
        fill(0.0, 0.0, 1.0),
        fill(1.0, 1.0, 1.0),
    );
    output.write(&edges).unwrap();
    assert_eq!(
        device.rgb(),
        [255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255],
        "left, top, right, bottom"
    );
    output.write(&EdgeColors::default()).unwrap();
    assert_eq!(device.rgb(), [0; 12]);
}

#[test]
fn idle_off_and_native_release_then_set_the_idle_state() {
    for (mode, state) in [
        (IdleMode::Off, off()),
        (IdleMode::Native, json!({"on": true, "lor": 2, "tt": 0})),
    ] {
        let device = Device::new(vec![Reply::Json(OK); 4]);
        let output = device.open(one_each()).unwrap();
        output.idle(mode, &Hangup::default()).unwrap();
        device.expect(&[release(), live(), release(), state]);
    }
}

/// A device nobody holds is put in its mode without live mode first, so it
/// never shows Prysm's colours meanwhile.
#[test]
fn idle_without_an_output_sets_the_mode_without_taking_the_device_over() {
    for (mode, state) in [
        (IdleMode::Off, off()),
        (IdleMode::Native, json!({"on": true, "lor": 2, "tt": 0})),
    ] {
        let device = Device::new(vec![Reply::Json(OK), Reply::Json(OK)]);
        let settings = device.settings(one_each(), true);
        device.wled.idle(&settings, mode).unwrap();
        device.expect(&[release(), state]);
    }
}

/// A request whose deadline has passed is not sent: the device's first
/// request is the one made after it, with time left.
#[test]
fn a_spent_deadline_sends_nothing() {
    let device = Device::new(vec![Reply::Json(OK)]);
    let spent = http::Deadline::after(Duration::ZERO);
    let error = http::post_state(
        device.http(),
        "wled",
        &json!({"on": true}),
        spent,
        &Hangup::default(),
    )
    .unwrap_err();
    assert!(
        matches!(error, RequestError::TimedOut(Duration::ZERO)),
        "{error:?}"
    );
    http::post_state(
        device.http(),
        "wled",
        &json!({"on": false}),
        http::Deadline::after(WAIT),
        &Hangup::default(),
    )
    .unwrap();
    assert_eq!(device.call(), json!({"on": false}));
}

/// Both requests of an idle share its one deadline, which a timeout names.
#[test]
fn an_idle_is_bounded_as_a_whole() {
    let device = Device::new(vec![Reply::Json(OK), Reply::Hang]);
    let settings = device.settings(one_each(), true);
    let error = device.wled.idle(&settings, IdleMode::Off).unwrap_err();
    assert!(
        matches!(request_error(&error), RequestError::TimedOut(IDLE_LIMIT)),
        "{error:?}"
    );
    device.expect(&[release(), off()]);
}

/// An open has one deadline: live mode by its first part, which a timeout
/// names, and turning the device off again by its end, [`OPEN_LIMIT`].
#[test]
fn an_open_is_bounded_as_a_whole() {
    let device = Device::new(vec![Reply::Hang, Reply::Hang]);
    let Err(error) = device.open(one_each()) else {
        panic!("live mode never answered");
    };
    let failed: &NotTurnedOff = source(&error).downcast_ref().expect("both failed");
    assert!(
        matches!(failed.live, RequestError::TimedOut(TIMEOUT)),
        "{failed:?}"
    );
    assert!(
        matches!(failed.off, RequestError::TimedOut(OPEN_LIMIT)),
        "{failed:?}"
    );
    device.expect(&[release(), release()]);
}

#[test]
fn unreachable_device_fails_open_with_its_cause() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let settings = WledSettings {
        address: format!("127.0.0.1:{port}"),
        leds: one_each(),
        dither: true,
    };
    let failure = Wled::local(4048)
        .open(&settings, &Hangup::default())
        .err()
        .unwrap();
    assert!(
        matches!(failure, OpenFailure::Untouched(_)),
        "nothing reached it"
    );
    let error = failure.into_error();
    assert!(error.to_string().contains("live mode"), "{error}");
    let cause = request_error(&error);
    assert!(matches!(cause, RequestError::Connect(_)), "{cause:?}");
    assert!(cause.source().is_some(), "io::Error retained");
}

#[test]
fn failed_live_request_reports_its_cause_and_turns_the_device_off() {
    let failures = [
        Reply::Raw("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".into()),
        Reply::Json(r#"{"success":false}"#),
        Reply::Json(r#"{"error":9}"#),
        Reply::Json(r#"{"on":true}"#),
        Reply::Json("not JSON"),
        Reply::Raw(String::new()),
        Reply::Raw("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{}".into()),
        Reply::Raw(format!("HTTP/1.1 200 OK\r\n\r\n{}", "x".repeat(4097))),
    ];
    for failure in failures {
        let device = Device::new(vec![
            Reply::Json(OK),
            failure,
            Reply::Json(OK),
            Reply::Json(OK),
        ]);
        let failure = device
            .wled
            .open(&device.settings(one_each(), true), &Hangup::default())
            .err()
            .unwrap();
        assert!(
            matches!(failure, OpenFailure::Touched(_)),
            "it took a request"
        );
        let error = failure.into_error();
        let cause = request_error(&error);
        assert!(!matches!(cause, RequestError::Connect(_)), "{cause:?}");
        assert!(!error.to_string().contains("also failed"), "{error}");
        device.expect(&[release(), live(), release(), off()]);
    }
}

#[test]
fn failed_compensation_is_reported_alongside_the_original_cause() {
    // Both causes carry a source of their own, so the text shows each chain
    // whole, in its own clause.
    let device = Device::new(vec![
        Reply::Json(OK),
        Reply::Json("nope"),
        Reply::Json(OK),
        Reply::Json("["),
    ]);
    let error = device.open(one_each()).err().unwrap();
    let both: &NotTurnedOff = source(&error).downcast_ref().expect("both causes");
    assert!(matches!(both.live, RequestError::Json(_)), "{both:?}");
    assert!(matches!(both.off, RequestError::Json(_)), "{both:?}");
    assert_eq!(
        prysm_ports::chain(&error),
        format!(
            "WLED at {} did not switch to live mode: response is not JSON: expected ident \
             at line 1 column 2; turning it off also failed: response is not JSON: EOF \
             while parsing a list at line 1 column 1",
            device.address("127.0.0.1")
        )
    );
}

#[test]
fn failed_live_mode_reads_as_one_sentence_with_its_cause() {
    let device = Device::new(vec![
        Reply::Json(OK),
        Reply::Json(r#"{"success":false}"#),
        Reply::Json(OK),
        Reply::Json(OK),
    ]);
    let error = device.open(one_each()).err().unwrap();
    assert_eq!(
        prysm_ports::chain(&error),
        format!(
            "WLED at {} did not switch to live mode: device rejected the request: \
             success is false",
            device.address("127.0.0.1")
        )
    );
}

#[test]
fn idle_failure_keeps_its_cause() {
    let device = Device::new(vec![
        Reply::Json(OK),
        Reply::Json(OK),
        Reply::Raw("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".into()),
    ]);
    let output = device.open(one_each()).unwrap();
    let error = output.idle(IdleMode::Off, &Hangup::default()).unwrap_err();
    assert!(error.to_string().contains("off mode"), "{error}");
    assert!(matches!(request_error(&error), RequestError::Status(404)));
    device.expect(&[release(), live(), release()]);
}

#[test]
fn chunked_and_close_delimited_replies_are_accepted() {
    let device = Device::new(vec![
        Reply::Raw(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n8\r\n{\"succes\r\n8\r\ns\":true}\r\n0\r\n\r\n"
                .into(),
        ),
        Reply::Raw(format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{OK}")),
    ]);
    device.open(one_each()).unwrap();
    device.expect(&[release(), live()]);
}

#[test]
fn addresses_parse_to_host_and_http_port() {
    let ip = |text: &str| Host::Ip(text.parse().unwrap());
    let name = |text: &str| Host::Name(text.into());
    for (text, host, port, display) in [
        ("192.168.1.50", ip("192.168.1.50"), 80, "192.168.1.50"),
        (
            "192.168.1.50:8080",
            ip("192.168.1.50"),
            8080,
            "192.168.1.50:8080",
        ),
        ("wled.local", name("wled.local"), 80, "wled.local"),
        (
            "WLED-Desk.local:8080",
            name("WLED-Desk.local"),
            8080,
            "WLED-Desk.local:8080",
        ),
        ("wled", name("wled"), 80, "wled"),
        ("wled.local:80", name("wled.local"), 80, "wled.local"),
        (
            "wled.local:65535",
            name("wled.local"),
            65535,
            "wled.local:65535",
        ),
        ("fe80::1", ip("fe80::1"), 80, "[fe80::1]"),
        ("[fe80::1]", ip("fe80::1"), 80, "[fe80::1]"),
        ("[fe80::1]:8080", ip("fe80::1"), 8080, "[fe80::1]:8080"),
        (
            "[::ffff:10.0.0.5]:81",
            ip("::ffff:10.0.0.5"),
            81,
            "[::ffff:10.0.0.5]:81",
        ),
    ] {
        let address = DeviceAddress::parse(text).unwrap();
        assert_eq!(address, DeviceAddress { host, port }, "{text}");
        assert_eq!(address.to_string(), display, "{text}");
    }
}

#[test]
fn validation_rejects_malformed_and_non_unicast_addresses_and_led_totals() {
    let valid = WledSettings {
        address: "192.168.1.50".into(),
        leds: one_each(),
        dither: true,
    };
    assert_eq!(Wled::default().validate(&valid), Ok(()));
    for (address, reason) in [
        ("", "required"),
        (" wled.local", "spaces"),
        ("wled local", "spaces"),
        ("https://wled.local", "plain HTTP"),
        ("HTTPS://wled.local:8080/", "plain HTTP"),
        ("http://wled.local:8080/", "scheme"),
        ("ws://wled.local", "scheme"),
        ("wled.local/json", "path"),
        ("wled.local?x", "path"),
        ("user@wled.local", "user information"),
        ("wled.local:", "port"),
        ("wled.local:0", "port"),
        ("wled.local:65536", "port"),
        ("wled.local:+80", "port"),
        ("wled.local:http", "port"),
        (":8080", "host name"),
        ("wled..local", "host name"),
        ("-wled.local", "host name"),
        ("wled_desk.local", "host name"),
        (&format!("{}.local", "a".repeat(64)), "host name"),
        ("256.1.1.1", "host name"),
        ("192.168.1", "host name"),
        ("fe80::1::8080", "brackets"),
        ("[fe80::1", "IPv6"),
        ("[fe80::1]8080", "IPv6"),
        ("[wled.local]:80", "IPv6"),
        ("0.0.0.0", "unicast"),
        ("255.255.255.255:80", "unicast"),
        ("224.0.0.1", "unicast"),
        ("::", "unicast"),
        ("[ff02::1]:80", "unicast"),
        ("::ffff:224.0.0.1", "unicast"),
        ("::ffff:255.255.255.255", "unicast"),
    ] {
        let settings = WledSettings {
            address: address.into(),
            ..valid.clone()
        };
        let error = settings.validate().unwrap_err();
        assert_eq!(error.field, "address", "{address}");
        assert!(error.reason.contains(reason), "{address}: {}", error.reason);
    }
    let with = |leds| {
        WledSettings {
            leds,
            ..valid.clone()
        }
        .validate()
    };
    // The total, at the counts: none, or more than WLED drives across edges.
    for leds in [
        LedCounts::default(),
        LedCounts {
            top: MAX_LEDS,
            left: 1,
            ..LedCounts::default()
        },
        LedCounts {
            top: 5000,
            right: 5000,
            bottom: 5000,
            left: 5000,
        },
    ] {
        let error = with(leds).unwrap_err();
        assert_eq!(error.field, "leds", "{leds:?}");
        assert_eq!(
            error.reason,
            "the LEDs must total 1 to 16384, the most WLED drives"
        );
    }
    // One edge over the limit, at that edge.
    for (leds, field) in [
        (
            LedCounts {
                top: MAX_LEDS + 1,
                ..LedCounts::default()
            },
            "leds.top",
        ),
        (
            LedCounts {
                top: 1,
                left: u16::MAX,
                ..LedCounts::default()
            },
            "leds.left",
        ),
    ] {
        let error = with(leds).unwrap_err();
        assert_eq!(error.field, field);
        assert_eq!(
            error.reason,
            "an edge can have at most 16384 LEDs, the most WLED drives"
        );
    }
    // Every LED on one edge, and 5000 on one edge, are fine.
    for leds in [
        LedCounts {
            top: MAX_LEDS,
            ..LedCounts::default()
        },
        LedCounts {
            top: 5000,
            right: 54,
            bottom: 96,
            left: 54,
        },
    ] {
        assert_eq!(with(leds), Ok(()), "{leds:?}");
    }
}

#[test]
fn custom_port_reaches_the_device_and_names_it_in_the_host_header() {
    let device = Device::new(vec![Reply::Json(OK); 2]);
    let _output = device.open(one_each()).unwrap();
    let (head, _) = device.call_with_head();
    let host = format!("\r\nHost: {}\r\n", device.address("127.0.0.1"));
    assert!(head.contains(&host), "{head}");
    device.expect(&[live()]);
}

#[test]
fn host_name_is_resolved_at_open_for_http_and_ddp() {
    let device = Device::new(vec![Reply::Json(OK); 2]);
    let settings = WledSettings {
        address: device.address("localhost"),
        leds: one_each(),
        dither: true,
    };
    let mut output = device.wled.open(&settings, &Hangup::default()).unwrap();
    let (head, state) = device.call_with_head();
    assert_eq!(state, release());
    let host = format!("\r\nHost: {}\r\n", device.address("localhost"));
    assert!(head.contains(&host), "{head}");
    device.expect(&[live()]);
    output.write(&EdgeColors::default()).unwrap();
    assert_eq!(device.rgb(), [0; 12], "DDP to the resolved IPv4 address");
}

#[test]
fn unresolvable_host_fails_open_naming_the_host() {
    let settings = WledSettings {
        address: "prysm-test.invalid:8080".into(),
        leds: one_each(),
        dither: true,
    };
    let wled = Wled::default()
        .with_lookup(|_, _| Err(io::Error::new(io::ErrorKind::NotFound, "no such host")));
    let failure = wled.open(&settings, &Hangup::default()).err().unwrap();
    assert!(
        matches!(failure, OpenFailure::Untouched(_)),
        "nothing reached it"
    );
    let error = failure.into_error();
    assert!(
        error
            .to_string()
            .contains("resolve WLED host prysm-test.invalid"),
        "{error}"
    );
    let Error::Adapter {
        source: Some(source),
        ..
    } = &error
    else {
        panic!("expected an adapter error with a source: {error:?}");
    };
    assert!(
        matches!(
            source.downcast_ref::<prysm_ports::CallError>(),
            Some(prysm_ports::CallError::Failed(_))
        ),
        "{source:?}"
    );
}

/// A name whose lookup hangs fails each open at the deadline, and the
/// reopens wait for the same lookup instead of starting new ones.
#[test]
fn a_hanging_lookup_is_bounded_and_not_repeated() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    // Opened once the test is done, so the lookup thread ends with it.
    static GATE: (std::sync::Mutex<bool>, std::sync::Condvar) =
        (std::sync::Mutex::new(false), std::sync::Condvar::new());
    let wled = Wled::default().with_lookup(|_, _| {
        CALLS.fetch_add(1, Ordering::SeqCst);
        let (open, opened) = &GATE;
        let _open = opened
            .wait_while(open.lock().unwrap(), |open| !*open)
            .unwrap();
        Err(io::Error::other("unreachable"))
    });
    let settings = WledSettings {
        address: "wled.local".into(),
        leds: one_each(),
        dither: true,
    };
    for _ in 0..2 {
        let error = wled
            .open(&settings, &Hangup::default())
            .err()
            .unwrap()
            .into_error();
        assert_eq!(
            prysm_ports::chain(&error),
            "cannot resolve WLED host wled.local: no answer yet"
        );
    }
    assert_eq!(CALLS.load(Ordering::SeqCst), 1, "one lookup at a time");
    *GATE.0.lock().unwrap() = true;
    GATE.1.notify_all();
}

#[test]
fn settings_file_shape_omits_nothing_but_defaults_missing_edges_and_dithering() {
    let settings: WledSettings =
        serde_json::from_value(json!({"address": "10.0.0.5", "leds": {"top": 96, "left": 54}}))
            .unwrap();
    assert_eq!(
        settings.leds,
        LedCounts {
            top: 96,
            left: 54,
            ..LedCounts::default()
        }
    );
    assert!(settings.dither);
    assert_eq!(
        serde_json::to_value(&settings).unwrap(),
        json!({
            "address": "10.0.0.5",
            "leds": {"top": 96, "right": 0, "bottom": 0, "left": 54},
            "dither": true
        })
    );
    for invalid in [
        json!({"address": "10.0.0.5", "leds": {"top": 1}, "port": 80}),
        json!({"address": "10.0.0.5", "leds": {"up": 1}}),
        json!({"address": 3232235826_u32, "leds": {"top": 1}}),
        json!({"address": "10.0.0.5", "leds": {"top": 65536}}),
    ] {
        assert!(serde_json::from_value::<WledSettings>(invalid).is_err());
    }
}

/// A `/json/si` reply; `healthy()` is WLED showing Prysm's DDP from 127.0.0.1.
fn si(state: Value, info: Value) -> Reply {
    let body = json!({ "state": state, "info": info }).to_string();
    Reply::Raw(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ))
}
fn healthy() -> Reply {
    sender("127.0.0.1")
}
fn sender(lip: &str) -> Reply {
    si(
        json!({"on": true, "lor": 0, "bri": 128, "seg": [{"id": 0}]}),
        json!({"live": true, "lm": "DDP", "lip": lip, "ver": "0.15.0"}),
    )
}

/// Run one check to its end with the clock at `now` (the reply is read as
/// it arrives); what it found.
fn check_at(observer: &mut Observer, now: Instant) -> Option<OutputIssue> {
    let issue = observer.poll(now);
    if !observer.checking() {
        return issue;
    }
    eventually("the check's reply", WAIT, || {
        let issue = observer.poll(now);
        (!observer.checking()).then_some(issue)
    })
}

/// Run one check to its end now.
fn check(observer: &mut Observer) -> Option<OutputIssue> {
    check_at(observer, Instant::now())
}

#[test]
fn each_takeover_cause_is_reported_and_clears_when_wled_shows_prysm_again() {
    let cases = [
        (healthy(), None),
        (
            si(
                json!({"on": false, "lor": 0}),
                json!({"live": true, "lm": "DDP", "lip": "127.0.0.1"}),
            ),
            Some(OutputIssue::Off),
        ),
        (healthy(), None),
        (
            si(
                json!({"on": true, "lor": 1}),
                json!({"live": true, "lm": "DDP", "lip": "127.0.0.1"}),
            ),
            Some(OutputIssue::Overridden),
        ),
        (
            si(
                json!({"on": true, "lor": 2}),
                json!({"live": false, "lm": "", "lip": ""}),
            ),
            Some(OutputIssue::Overridden),
        ),
        (
            si(
                json!({"on": true, "lor": 0}),
                json!({"live": false, "lm": "", "lip": ""}),
            ),
            Some(OutputIssue::NotLive),
        ),
        // Live only through Prysm's JSON request: no DDP packet arrived.
        (
            si(
                json!({"on": true, "lor": 0}),
                json!({"live": true, "lm": "", "lip": ""}),
            ),
            Some(OutputIssue::NotLive),
        ),
        (healthy(), None),
        (sender("192.0.2.9"), Some(OutputIssue::OtherSender)),
        (
            si(
                json!({"on": true, "lor": 0}),
                json!({"live": true, "lm": "Hyperion", "lip": "192.0.2.9"}),
            ),
            Some(OutputIssue::OtherSender),
        ),
        (healthy(), None),
        (Reply::Raw(String::new()), Some(OutputIssue::Unreachable)),
        (
            Reply::Raw("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".into()),
            Some(OutputIssue::Unreachable),
        ),
        (
            Reply::Json(r#"{"state":{}}"#),
            Some(OutputIssue::Unreachable),
        ),
        // A sender address that cannot be read names no other sender.
        (sender("not an address"), None),
        // Firmware without `lor`, `lm` or `lip` reports what it can.
        (si(json!({"on": true}), json!({"live": true})), None),
    ];
    let (replies, expected): (Vec<_>, Vec<_>) = cases.into_iter().unzip();
    let device = Device::new(replies);
    let mut observer = device.observer();
    for (index, expected) in expected.into_iter().enumerate() {
        assert_eq!(check(&mut observer), expected, "check {index}");
        device.check_request();
    }
}

#[test]
fn the_sender_address_behind_nat_is_learned_from_wled() {
    // Docker's bridge network: WLED sees the host's address, not ours.
    let device = Device::new(vec![
        sender("10.0.0.2"),
        sender("10.0.0.2"),
        sender("10.0.0.3"),
        sender("10.0.0.2"),
    ]);
    let mut observer = device.observer();
    let found: Vec<_> = (0..4).map(|_| check(&mut observer)).collect();
    assert_eq!(found, [None, None, Some(OutputIssue::OtherSender), None]);

    // Another sender was showing first; our own address corrects the guess.
    let device = Device::new(vec![sender("192.0.2.9"), healthy(), sender("192.0.2.9")]);
    let mut observer = device.observer();
    let found: Vec<_> = (0..3).map(|_| check(&mut observer)).collect();
    assert_eq!(found, [None, None, Some(OutputIssue::OtherSender)]);
}

/// A refused connect is reported as soon as a poll sees it, not at the
/// reply's deadline: the clock never moves here.
#[test]
fn an_unreachable_state_endpoint_is_reported_at_once() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let device = listener.local_addr().unwrap();
    drop(listener);
    let t0 = Instant::now();
    let mut observer = Observer::new(device, "wled", Duration::ZERO, t0);
    let found = eventually("the refusal", WAIT, || {
        let found = observer.poll(t0);
        (!observer.checking()).then_some(found)
    });
    assert_eq!(found, Some(OutputIssue::Unreachable));
}

/// A device that does not answer the connect (a firewall dropping TCP 80,
/// say) does not hold the caller: the poll that starts the check returns
/// with it connecting, later polls go on, and the check is given up at
/// [`REPLY`]. A listener whose queue is full stands in: Linux lets the
/// connects wait (macOS refuses them, so the test is Linux's).
#[cfg(target_os = "linux")]
#[test]
fn a_state_check_never_waits_for_the_connect() {
    let listener =
        socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
    listener
        .bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())
        .unwrap();
    listener.listen(0).unwrap();
    let address = listener.local_addr().unwrap().as_socket().unwrap();
    // Connects nobody accepts, until one is left waiting: the queue is full.
    let mut fillers = Vec::new();
    loop {
        let filler = std::net::TcpStream::connect_timeout(&address, Duration::from_millis(100));
        match filler {
            Ok(filler) => fillers.push(filler),
            Err(_) => break,
        }
        assert!(fillers.len() < 64, "the queue never filled");
    }
    let t0 = Instant::now();
    let mut observer = Observer::new(address, "wled", Duration::ZERO, t0);
    assert_eq!(observer.poll(t0), None, "the check started");
    assert!(observer.checking(), "still connecting");
    assert_eq!(observer.poll(t0 + REPLY / 2), None);
    assert!(observer.checking(), "still connecting");
    assert_eq!(observer.poll(t0 + REPLY), Some(OutputIssue::Unreachable));
    assert!(!observer.checking());
}

/// State checks allow 64 KiB, more than the 20 KiB of other requests:
/// segments and usermods make `/json/si` large.
#[test]
fn a_state_reply_may_hold_up_to_64_kib() {
    let state = |pad: usize| {
        si(
            json!({"on": true, "lor": 0, "pad": "x".repeat(pad)}),
            json!({"live": true, "lm": "DDP", "lip": "127.0.0.1"}),
        )
    };
    let device = Device::new(vec![state(40 * 1024), state(64 * 1024)]);
    let mut observer = device.observer();
    assert_eq!(check(&mut observer), None);
    assert_eq!(check(&mut observer), Some(OutputIssue::Unreachable));
}

/// A reply that arrives a byte at a time cannot keep a check open past its
/// deadline, [`REPLY`] after the request.
#[test]
fn a_dripping_state_reply_is_abandoned_at_the_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (dripped, head_written) = mpsc::channel();
    thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let _ = socket.read(&mut [0; 1024]);
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\n";
        for (written, byte) in head.iter().chain(std::iter::repeat(&b' ')).enumerate() {
            if socket.write_all(&[*byte]).is_err() {
                return;
            }
            if written + 1 == head.len() {
                let _ = dripped.send(());
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
    let t0 = Instant::now();
    let mut observer = Observer::new(address, "wled", Duration::ZERO, t0);
    // The whole head drips in, a byte at a time; the body never ends.
    eventually("the reply to begin", WAIT, || {
        assert_eq!(observer.poll(t0), None);
        assert!(observer.checking());
        head_written.try_recv().ok()
    });
    let before = t0 + REPLY - Duration::from_millis(1);
    assert_eq!(observer.poll(before), None);
    assert!(observer.checking(), "still reading before the deadline");
    assert_eq!(observer.poll(t0 + REPLY), Some(OutputIssue::Unreachable));
    assert!(!observer.checking());
}

/// After a usable state the next check is [`EVERY`] later; after none,
/// [`BACKOFF`] times that.
#[test]
fn state_checks_wait_every_and_back_off_after_no_answer() {
    let every = EVERY;
    let just = Duration::from_millis(1);
    let device = Device::new(vec![healthy(), Reply::Raw(String::new()), healthy()]);
    let t0 = Instant::now();
    let mut observer =
        Observer::new(device.http(), &device.address("127.0.0.1"), every, t0).state_only();
    let mut answered = t0;
    // The first check is `every` after the observer starts.
    for (wait, found) in [
        (every, None),
        (every, Some(OutputIssue::Unreachable)),
        (every * BACKOFF, None),
    ] {
        let due = answered + wait;
        observer.poll(due - just);
        assert!(!observer.checking(), "a check before it was due");
        assert_eq!(check_at(&mut observer, due), found);
        device.check_request();
        answered = due;
    }
}

/// A configuration read that WLED refuses backs the next check off too.
#[test]
fn a_refused_configuration_read_backs_off() {
    let every = EVERY;
    let device = Device::new(vec![at(255), refused(), at(255)]);
    let t0 = Instant::now();
    let mut observer = Observer::new(device.http(), &device.address("127.0.0.1"), every, t0);
    let first = t0 + every;
    assert_eq!(check_at(&mut observer, first), None);
    device.check_request();
    let second = first + every;
    assert_eq!(
        check_at(&mut observer, second),
        None,
        "a refusal is no issue"
    );
    device.get_request("/json/cfg");
    let third = second + every * BACKOFF;
    observer.poll(third - Duration::from_millis(1));
    assert!(!observer.checking(), "backed off");
    assert_eq!(check_at(&mut observer, third), None);
    device.check_request();
}

#[test]
fn a_silent_state_endpoint_neither_stalls_nor_fails_frame_delivery() {
    let device = Device::new(vec![Reply::Json(OK), Reply::Json(OK), Reply::Hang]);
    let mut output = device.open(one_each()).unwrap();
    device.expect(&[release(), live()]);
    // The check is sent and its reply never comes; frames go on meanwhile.
    // (That a check never waits, and ends at its deadline, the observer's
    // own tests show with injected time.)
    device.check_sent(|| {
        output.write(&EdgeColors::default()).unwrap();
        assert_eq!(device.rgb(), [0; 12], "every frame reaches WLED");
        assert!(output.report().issue.is_none());
    });
    for _ in 0..20 {
        output.write(&EdgeColors::default()).unwrap();
        assert_eq!(device.rgb(), [0; 12], "every frame reaches WLED");
        // No answer yet, or none by the deadline.
        let issue = output.report().issue;
        assert!(
            matches!(issue, None | Some(OutputIssue::Unreachable)),
            "{issue:?}"
        );
    }
}

#[test]
fn idle_abandons_a_check_in_flight_and_restores_the_device() {
    for (mode, state) in [
        (IdleMode::Off, off()),
        (IdleMode::Native, json!({"on": true, "lor": 2, "tt": 0})),
    ] {
        let device = Device::new(vec![
            Reply::Json(OK),
            Reply::Json(OK),
            Reply::Hang,
            Reply::Json(OK),
            Reply::Json(OK),
        ]);
        let mut output = device.open(one_each()).unwrap();
        device.expect(&[release(), live()]);
        output.write(&EdgeColors::default()).unwrap();
        device.check_sent(|| assert_eq!(output.report().issue, None, "no answer yet"));
        // The fixture answers one connection at a time: the idle's requests
        // are answered only once the check's connection is closed, so an
        // idle that waited for the check would time out instead.
        output.idle(mode, &Hangup::default()).unwrap();
        device.expect(&[release(), state]);
    }
}

#[test]
fn findings_survive_until_the_next_check_and_idle_still_restores() {
    let device = Device::new(vec![
        Reply::Json(OK),
        Reply::Json(OK),
        si(
            json!({"on": false, "lor": 0}),
            json!({"live": true, "lm": "DDP", "lip": "127.0.0.1"}),
        ),
        Reply::Json(OK),
        Reply::Json(OK),
    ]);
    let mut output = device.open(one_each()).unwrap();
    device.expect(&[release(), live()]);
    eventually("WLED switched off to be reported", WAIT, || {
        output.write(&EdgeColors::default()).unwrap();
        (output.report().issue == Some(OutputIssue::Off)).then_some(())
    });
    device.check_request();
    output.idle(IdleMode::Off, &Hangup::default()).unwrap();
    device.expect(&[release(), off()]);
}

/// A `/json/cfg` reply with WLED's realtime and LED brightness settings.
fn cfg(body: Value) -> Reply {
    let body = body.to_string();
    Reply::Raw(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ))
}
/// WLED showing Prysm's DDP at master brightness `bri`.
fn at(bri: u8) -> Reply {
    si(
        json!({"on": true, "lor": 0, "bri": bri}),
        json!({"live": true, "lm": "DDP", "lip": "127.0.0.1"}),
    )
}
fn percent(percent: u8) -> Option<DeviceBrightness> {
    DeviceBrightness::from_percent(percent)
}

#[test]
fn wled_brightness_is_reported_once_its_configuration_says_it_applies() {
    let cases = [
        // WLED's defaults: master brightness applies, factor 100 %.
        (
            cfg(
                json!({"if": {"live": {"en": true, "maxbri": false}}, "light": {"scale-bri": 100}}),
            ),
            102,
            percent(40),
        ),
        // A configuration without the fields takes WLED's defaults.
        (cfg(json!({"if": {"live": {}}})), 102, percent(40)),
        // "Force max brightness": realtime colours ignore `bri`.
        (
            cfg(json!({"if": {"live": {"maxbri": true}}, "light": {"scale-bri": 100}})),
            102,
            percent(100),
        ),
        // The brightness factor scales either way, capped at full.
        (
            cfg(json!({"if": {"live": {"maxbri": true}}, "light": {"scale-bri": 50}})),
            255,
            percent(50),
        ),
        (
            cfg(json!({"if": {"live": {"maxbri": false}}, "light": {"scale-bri": 200}})),
            102,
            percent(80),
        ),
        (
            cfg(json!({"if": {"live": {"maxbri": false}}, "light": {"scale-bri": 200}})),
            200,
            percent(100),
        ),
        // "Use Gamma correction for brightness": `bri` goes through the
        // gamma table first (102 becomes 34 at 2.2), then the factor.
        (
            cfg(json!({"if": {"live": {}}, "light": {"gc": {"bri": 2.2}}})),
            102,
            percent(13),
        ),
        (
            cfg(json!({"if": {"live": {}}, "light": {"gc": {"bri": 2.2}, "scale-bri": 200}})),
            102,
            percent(27),
        ),
        // Forced to full, the table leaves it full.
        (
            cfg(json!({"if": {"live": {"maxbri": true}}, "light": {"gc": {"bri": 2.2}}})),
            102,
            percent(100),
        ),
        // Behind WLED's settings PIN: nothing to go on, nothing reported.
        (
            Reply::Raw("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".into()),
            102,
            None,
        ),
    ];
    for (index, (config, bri, expected)) in cases.into_iter().enumerate() {
        let device = Device::new(vec![at(bri), config, at(bri)]);
        let mut observer = device.observer_with_config();
        assert_eq!(check(&mut observer), None, "case {index}");
        assert_eq!(
            observer.brightness(),
            None,
            "case {index}: no configuration yet"
        );
        device.check_request();
        // The second check reads the configuration, leaving the finding.
        assert_eq!(check(&mut observer), None, "case {index}");
        device.get_request("/json/cfg");
        assert_eq!(observer.brightness(), expected, "case {index}");
        assert_eq!(check(&mut observer), None, "case {index}");
        device.check_request();
        assert_eq!(observer.brightness(), expected, "case {index}");
    }
}

#[test]
fn wled_brightness_follows_the_state_and_goes_with_an_unanswered_check() {
    let device = Device::new(vec![
        at(255),
        cfg(json!({"if": {"live": {"maxbri": false}}})),
        at(64),
        Reply::Raw(String::new()),
    ]);
    let mut observer = device.observer_with_config();
    check(&mut observer);
    check(&mut observer);
    assert_eq!(observer.brightness(), percent(100));
    check(&mut observer);
    assert_eq!(observer.brightness(), percent(25), "64 of 255");
    assert_eq!(check(&mut observer), Some(OutputIssue::Unreachable));
    assert_eq!(observer.brightness(), None, "no state, no brightness");
    // The colours are still encoded for the brightness WLED last had: one
    // missed check does not rebuild the dither levels for full brightness.
    assert_eq!(observer.level(), Some(64));
}

#[test]
fn an_open_output_reports_wled_brightness() {
    let device = Device::new(vec![
        Reply::Json(OK),
        Reply::Json(OK),
        at(102),
        cfg(json!({"if": {"live": {"maxbri": false}}})),
        Reply::Json(OK),
        Reply::Json(OK),
    ]);
    let mut output = device.open(one_each()).unwrap();
    device.expect(&[release(), live()]);
    // A report polls, so each starts the next check: the answers are read
    // from the report that had them.
    let brightness = eventually("WLED brightness to be reported", WAIT, || {
        output.write(&EdgeColors::default()).unwrap();
        let report = output.report();
        assert_eq!(report.issue, None);
        report.brightness
    });
    assert_eq!(Some(brightness), percent(40));
    device.check_request();
    device.get_request("/json/cfg");
    output.idle(IdleMode::Off, &Hangup::default()).unwrap();
    device.expect(&[release(), off()]);
}

/// WLED showing Prysm's DDP while it drives `count` LEDs.
fn with_leds(count: u32) -> Reply {
    si(
        json!({"on": true, "lor": 0, "bri": 128}),
        json!({"live": true, "lm": "DDP", "lip": "127.0.0.1", "leds": {"count": count, "fps": 42}}),
    )
}

#[test]
fn wled_led_count_follows_the_state_and_goes_with_an_unanswered_check() {
    let device = Device::new(vec![
        with_leds(120),
        with_leds(96),
        healthy(),
        with_leds(120),
        Reply::Raw(String::new()),
    ]);
    let mut observer = device.observer();
    assert_eq!(observer.led_count(), None, "nothing asked yet");
    assert_eq!(check(&mut observer), None, "a count is no issue");
    assert_eq!(observer.led_count(), Some(120));
    check(&mut observer);
    assert_eq!(observer.led_count(), Some(96), "changed in WLED");
    check(&mut observer);
    assert_eq!(observer.led_count(), None, "a state without one");
    check(&mut observer);
    assert_eq!(observer.led_count(), Some(120));
    assert_eq!(check(&mut observer), Some(OutputIssue::Unreachable));
    assert_eq!(observer.led_count(), None, "no state, no count");
}

#[test]
fn an_open_output_reports_wled_led_count_and_keeps_sending() {
    let device = Device::new(vec![
        Reply::Json(OK),
        Reply::Json(OK),
        with_leds(120),
        Reply::Json(OK),
        Reply::Json(OK),
    ]);
    let mut output = device.open(one_each()).unwrap();
    device.expect(&[release(), live()]);
    let mut reports = 0;
    let count = eventually("the WLED LED count to be reported", WAIT, || {
        output.write(&EdgeColors::default()).unwrap();
        let report = output.report();
        reports += 1;
        assert_eq!(report.issue, None);
        report.led_count
    });
    assert!(reports > 1, "unknown before the check answered");
    // Four LEDs sent, 120 on the device: reported, and writing goes on.
    assert_eq!(count, 120);
    output.write(&EdgeColors::default()).unwrap();
    device.check_request();
    output.idle(IdleMode::Off, &Hangup::default()).unwrap();
    device.expect(&[release(), off()]);
}

/// WLED 16.0.1's own `/json/cfg`, cut to what Prysm reads, with colour gamma
/// `col` and "Disable realtime gamma correction" `no_gc`.
fn gamma_cfg(col: f32, no_gc: bool) -> Reply {
    cfg(json!({
        "light": {"gc": {"bri": 1, "col": col, "val": 2.2}, "scale-bri": 100},
        "if": {"live": {"en": true, "maxbri": false, "no-gc": no_gc}},
    }))
}
fn gamma(value: f32) -> Transfer {
    Transfer::Gamma(Gamma::new(value).unwrap())
}
fn refused() -> Reply {
    Reply::Raw("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".into())
}

#[test]
fn wled_gamma_is_read_from_its_configuration() {
    let linear = Transfer::Gamma(Gamma::LINEAR);
    let cases = [
        (gamma_cfg(2.2, false), gamma(2.2)),
        (gamma_cfg(2.8, false), gamma(2.8)),
        // WLED 16 takes gammas under 1.
        (gamma_cfg(0.5, false), gamma(0.5)),
        // "Use Gamma correction for color" off: WLED reports 1.
        (gamma_cfg(1.0, false), linear),
        // Realtime gamma disabled: linear, whatever the colour gamma.
        (gamma_cfg(2.8, true), linear),
        // No colour gamma given: unknown.
        (cfg(json!({"if": {"live": {}}})), Transfer::Srgb),
        (
            cfg(json!({"if": {"live": {}}, "light": {"gc": {"bri": 1}}})),
            Transfer::Srgb,
        ),
        // Behind WLED's settings PIN.
        (refused(), Transfer::Srgb),
    ];
    for (index, (config, expected)) in cases.into_iter().enumerate() {
        let device = Device::new(vec![at(255), config]);
        let mut observer = device.observer_with_config();
        check(&mut observer);
        assert_eq!(
            observer.transfer(),
            Transfer::Srgb,
            "case {index}: not read yet"
        );
        device.check_request();
        check(&mut observer);
        device.get_request("/json/cfg");
        assert_eq!(observer.transfer(), expected, "case {index}");
    }
}

/// A captured WLED 16.0.1 `/json/cfg`, whole, with its network identity
/// (names, SSIDs, addresses, MQTT) replaced: colour gamma 2.2, brightness
/// gamma off, realtime gamma correction on, brightness factor 100 %.
const WLED_16_CFG: &str = include_str!("../tests/fixtures/wled-16.0.1-cfg.json");

#[test]
fn a_real_wled_configuration_gives_its_gamma() {
    let device = Device::new(vec![at(102), Reply::Json(WLED_16_CFG)]);
    let mut observer = device.observer_with_config();
    check(&mut observer);
    device.check_request();
    check(&mut observer);
    device.get_request("/json/cfg");
    assert_eq!(observer.transfer(), gamma(2.2));
    // Brightness gamma off: 102 of 255 is 40 %; through 2.2 it would be 13 %.
    assert_eq!(observer.brightness(), percent(40));
}

#[test]
fn a_gamma_changed_in_wled_is_followed_and_kept_through_a_refused_read() {
    // Checks 1, 31 and 61 (from 0) read the configuration; the last is refused.
    let states = || (0..29).map(|_| at(255));
    let replies = [at(255), gamma_cfg(2.2, false)]
        .into_iter()
        .chain(states())
        .chain([gamma_cfg(2.8, false)])
        .chain(states())
        .chain([refused()])
        .collect();
    let device = Device::new(replies);
    let mut observer = device.observer_with_config();
    let mut checks = 0;
    for (until, expected) in [
        (2, gamma(2.2)),
        (31, gamma(2.2)),
        (32, gamma(2.8)),
        (62, gamma(2.8)),
    ] {
        while checks < until {
            check(&mut observer);
            checks += 1;
        }
        assert_eq!(observer.transfer(), expected, "after {checks} checks");
    }
}

#[test]
fn an_open_output_encodes_for_wled_gamma() {
    // 0.18 under WLED's gamma 2.8 is 45.9 of 255: undithered the nearest
    // byte, 0.18^(1/2.8); dithered the bytes of levels 45 and 46 around it.
    for (dither, reds) in [(false, &[138][..]), (true, &[137, 138][..])] {
        let device = Device::new(vec![
            Reply::Json(OK),
            Reply::Json(OK),
            at(255),
            gamma_cfg(2.8, false),
            Reply::Json(OK),
            Reply::Json(OK),
        ]);
        let mut output = device.open_with(one_each(), dither).unwrap();
        device.expect(&[release(), live()]);
        eventually("the WLED configuration to be read", WAIT, || {
            let report = output.report();
            assert_eq!(report.issue, None);
            report.brightness
        });
        let color = fill(0.18, 0.0, 1.0);
        output
            .write(&EdgeColors::new(
                color.clone(),
                color.clone(),
                color.clone(),
                color,
            ))
            .unwrap();
        for led in device.rgb().chunks(3) {
            assert!(reds.contains(&led[0]), "dither {dither}: {led:?}");
            assert_eq!(&led[1..], [0, 255], "dither {dither}");
        }
        device.check_request();
        device.get_request("/json/cfg");
        output.idle(IdleMode::Off, &Hangup::default()).unwrap();
        device.expect(&[release(), off()]);
    }
}

/// The bound of a call the cut ends.
const AT_ONCE: Duration = Duration::from_secs(1);

/// Whether `error` says a cut ended it.
fn stopped(error: &Error) -> bool {
    error.cause::<prysm_ports::Stopping>().is_some()
}

/// Wait (yielding) until `hangup` counts a blocking wait.
fn blocked(hangup: &Hangup) {
    let hangup = hangup.clone();
    prysm_testkit::within(WAIT, move || {
        while hangup.waits() == 0 {
            thread::yield_now();
        }
    });
}

/// Open on its own thread with a [`Hangup`], cut it once `ready` returns,
/// and the failure, within [`AT_ONCE`] of the cut.
fn cut_open(wled: Wled, settings: WledSettings, ready: impl FnOnce(&Hangup)) -> OpenFailure {
    let hangup = Hangup::default();
    let opening = {
        let hangup = hangup.clone();
        thread::spawn(move || wled.open(&settings, &hangup).err())
    };
    ready(&hangup);
    hangup.cut();
    prysm_testkit::within(AT_ONCE, move || opening.join().unwrap()).expect("the open failed")
}

/// A cut ends an open waiting for its first reply at once: the request may
/// have reached the device, so it is touched, and it is not turned off
/// again (whoever cut it sets the mode).
#[test]
fn a_cut_ends_an_open_waiting_for_a_reply() {
    let device = Device::new(vec![Reply::Hang]);
    let settings = device.settings(one_each(), true);
    let wled = Wled::local(device.udp.local_addr().unwrap().port());
    let failure = cut_open(wled, settings, |_| assert_eq!(device.call(), release()));
    let OpenFailure::Touched(error) = failure else {
        panic!("the request may have reached it");
    };
    assert!(stopped(&error), "{}", prysm_ports::chain(&error));
    device.expect(&[]);
}

/// An open after the cut (one that follows the frame a stopping session's
/// pull returned) fails at once without connecting: nothing reached the
/// device. (capture-remote's tests show a cut waking a connect in
/// progress, through the same `prysm_ports::Hangup::connect`.)
#[test]
fn an_open_after_the_cut_reaches_nothing() {
    let device = Device::new(Vec::new());
    let hangup = Hangup::default();
    hangup.cut();
    let failure = device
        .wled
        .open(&device.settings(one_each(), true), &hangup)
        .err()
        .expect("the open failed");
    let OpenFailure::Untouched(error) = failure else {
        panic!("nothing reached it");
    };
    assert!(stopped(&error), "{}", prysm_ports::chain(&error));
    device.expect(&[]);
}

/// A cut ends an open waiting for its name to resolve at once.
#[test]
fn a_cut_ends_an_open_waiting_for_its_name() {
    // Opened once the test is done, so the lookup thread ends with it.
    static GATE: (std::sync::Mutex<bool>, std::sync::Condvar) =
        (std::sync::Mutex::new(false), std::sync::Condvar::new());
    let wled = Wled::default().with_lookup(|_, _| {
        let (open, opened) = &GATE;
        let _open = opened
            .wait_while(open.lock().unwrap(), |open| !*open)
            .unwrap();
        Err(io::Error::other("too late"))
    });
    let settings = WledSettings {
        address: "wled.local".into(),
        leds: one_each(),
        dither: true,
    };
    let OpenFailure::Untouched(error) = cut_open(wled, settings, blocked) else {
        panic!("nothing reached it");
    };
    assert!(stopped(&error), "{}", prysm_ports::chain(&error));
    *GATE.0.lock().unwrap() = true;
    GATE.1.notify_all();
}

/// A cut ends a held output's idle waiting for its reply at once.
#[test]
fn a_cut_ends_an_idle_waiting_for_a_reply() {
    let device = Device::new(vec![Reply::Json(OK), Reply::Json(OK), Reply::Hang]);
    let output = device.open(one_each()).unwrap();
    device.expect(&[release(), live()]);
    let hangup = Hangup::default();
    let idling = {
        let hangup = hangup.clone();
        thread::spawn(move || output.idle(IdleMode::Native, &hangup))
    };
    assert_eq!(device.call(), release());
    hangup.cut();
    let error = prysm_testkit::within(AT_ONCE, move || idling.join().unwrap()).unwrap_err();
    assert!(stopped(&error), "{}", prysm_ports::chain(&error));
}
