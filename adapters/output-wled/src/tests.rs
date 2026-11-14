//! Loopback fixtures stand in for the device: a scripted HTTP listener and a UDP socket.
use crate::address::{DeviceAddress, Host};
use crate::observe::Observer;
use crate::{LedCounts, MAX_LEDS, Wled, WledSettings, http::RequestError};
use prysm_core::{ColorStrip, EdgeColors, LinearColor};
use prysm_ports::{
    AdapterSettings, Error, IdleMode, LedOutput, OutputFactory, OutputIssue, Registry,
};
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
                let deadline = Instant::now() + WAIT;
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "no connection");
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
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

    fn observer(&self) -> Observer {
        let device = SocketAddr::from(([127, 0, 0, 1], self.http_port));
        Observer::new(device, &self.address("127.0.0.1"), Duration::ZERO)
    }

    /// The next request must be a state check.
    fn check_request(&self) {
        let (head, _) = self.calls.recv_timeout(WAIT).expect("no state check");
        assert!(head.starts_with("GET /json/si HTTP/1.1\r\n"), "{head}");
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
        let settings = WledSettings {
            address: self.address("127.0.0.1"),
            leds,
        };
        self.wled.open(&settings)
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

fn request_error(error: &Error) -> &RequestError {
    let Error::Adapter {
        source: Some(source),
        ..
    } = error
    else {
        panic!("expected an adapter error with a source: {error:?}");
    };
    source.downcast_ref().expect("HTTP request error")
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
        },
    );
    let _output = registry.open_output(&settings).unwrap();
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
        output.idle(mode).unwrap();
        device.expect(&[release(), live(), release(), state]);
    }
}

#[test]
fn unreachable_device_fails_open_with_its_cause() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let settings = WledSettings {
        address: format!("127.0.0.1:{port}"),
        leds: one_each(),
    };
    let error = Wled::local(4048).open(&settings).err().unwrap();
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
        let error = device.open(one_each()).err().unwrap();
        let cause = request_error(&error);
        assert!(!matches!(cause, RequestError::Connect(_)), "{cause:?}");
        assert!(!error.to_string().contains("also failed"), "{error}");
        device.expect(&[release(), live(), release(), off()]);
    }
}

#[test]
fn failed_compensation_is_reported_alongside_the_original_cause() {
    let device = Device::new(vec![
        Reply::Json(OK),
        Reply::Json(r#"{"success":false}"#),
        Reply::Json(OK),
        Reply::Raw("HTTP/1.1 503 Busy\r\nContent-Length: 0\r\n\r\n".into()),
    ]);
    let error = device.open(one_each()).err().unwrap();
    assert!(matches!(request_error(&error), RequestError::Rejected(_)));
    assert!(error.to_string().contains("off also failed"), "{error}");
}

#[test]
fn idle_failure_keeps_its_cause() {
    let device = Device::new(vec![
        Reply::Json(OK),
        Reply::Json(OK),
        Reply::Raw("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".into()),
    ]);
    let output = device.open(one_each()).unwrap();
    let error = output.idle(IdleMode::Off).unwrap_err();
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
    };
    let mut output = device.wled.open(&settings).unwrap();
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
    };
    let started = Instant::now();
    let error = Wled::default().open(&settings).err().unwrap();
    assert!(started.elapsed() < Duration::from_secs(3), "bounded");
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
    assert!(source.downcast_ref::<io::Error>().is_some(), "{source:?}");
}

/// A name whose lookup hangs fails each open at the deadline, and the
/// reopens wait for the same lookup instead of starting new ones.
#[test]
fn a_hanging_lookup_is_bounded_and_not_repeated() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    let wled = Wled::default().with_lookup(|_, _| {
        CALLS.fetch_add(1, Ordering::SeqCst);
        thread::sleep(Duration::from_secs(60));
        Err(io::Error::other("unreachable"))
    });
    let settings = WledSettings {
        address: "wled.local".into(),
        leds: one_each(),
    };
    for _ in 0..2 {
        let error = wled.open(&settings).err().unwrap();
        assert_eq!(
            prysm_ports::chain(&error),
            "cannot resolve WLED host wled.local: no answer yet"
        );
    }
    assert_eq!(CALLS.load(Ordering::SeqCst), 1, "one lookup at a time");
}

#[test]
fn settings_file_shape_omits_nothing_but_defaults_missing_edges() {
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
    assert_eq!(
        serde_json::to_value(&settings).unwrap(),
        json!({"address": "10.0.0.5", "leds": {"top": 96, "right": 0, "bottom": 0, "left": 54}})
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

/// Run one state check to its end; what it found.
fn check(observer: &mut Observer) -> Option<OutputIssue> {
    let deadline = Instant::now() + WAIT;
    let mut issue = observer.poll();
    while observer.checking() {
        assert!(Instant::now() < deadline, "check never finished");
        thread::sleep(Duration::from_millis(2));
        issue = observer.poll();
    }
    issue
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

#[test]
fn an_unreachable_state_endpoint_is_reported_at_once() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let device = listener.local_addr().unwrap();
    drop(listener);
    let mut observer = Observer::new(device, "wled", Duration::ZERO);
    let started = Instant::now();
    assert_eq!(observer.poll(), Some(OutputIssue::Unreachable));
    assert!(!observer.checking());
    assert!(started.elapsed() < Duration::from_millis(250));
}

#[test]
fn a_silent_state_endpoint_neither_stalls_nor_fails_frame_delivery() {
    let device = Device::new(vec![Reply::Json(OK), Reply::Json(OK), Reply::Hang]);
    let mut output = device.open(one_each()).unwrap();
    device.expect(&[release(), live()]);
    let started = Instant::now();
    let mut slowest = Duration::ZERO;
    let mut frames = 0;
    let issue = loop {
        let call = Instant::now();
        output.write(&EdgeColors::default()).unwrap();
        let issue = output.observe();
        slowest = slowest.max(call.elapsed());
        assert_eq!(device.rgb(), [0; 12], "every frame reaches WLED");
        frames += 1;
        if issue.is_some() {
            break issue;
        }
        assert!(started.elapsed() < WAIT, "no verdict");
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(issue, Some(OutputIssue::Unreachable));
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "waited for the reply"
    );
    // Loose bounds for loaded runners: what matters is that no call waits
    // for the 1.5 s state request.
    assert!(frames > 10, "{frames} frames");
    assert!(slowest < Duration::from_millis(500), "{slowest:?}");
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
        assert_eq!(output.observe(), None, "check sent, no answer yet");
        device.check_request();
        let started = Instant::now();
        output.idle(mode).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
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
    let deadline = Instant::now() + WAIT;
    while output.observe() != Some(OutputIssue::Off) {
        assert!(
            Instant::now() < deadline,
            "WLED switched off was not reported"
        );
        output.write(&EdgeColors::default()).unwrap();
        thread::sleep(Duration::from_millis(2));
    }
    device.check_request();
    output.idle(IdleMode::Off).unwrap();
    device.expect(&[release(), off()]);
}
