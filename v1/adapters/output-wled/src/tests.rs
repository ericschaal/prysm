//! Loopback fixtures stand in for the device: a scripted HTTP listener and a UDP socket.
use crate::{LedCounts, Wled, WledSettings, http::RequestError};
use prysm_core::{ColorStrip, EdgeColors, LinearColor};
use prysm_ports::{AdapterSettings, Error, IdleMode, LedOutput, OutputFactory, Registry};
use serde_json::{Value, json};
use std::error::Error as _;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::num::NonZeroUsize;
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
}

struct Device {
    wled: Wled,
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
        let wled = Wled::local(
            listener.local_addr().unwrap().port(),
            udp.local_addr().unwrap().port(),
        );
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
                };
                let _ = socket.write_all(response.as_bytes());
            }
        });
        Self { wled, udp, calls }
    }

    fn call(&self) -> Value {
        let (head, json) = self.calls.recv_timeout(WAIT).expect("no HTTP request");
        assert!(head.starts_with("POST /json/state HTTP/1.1\r\n"), "{head}");
        json
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
            address: "127.0.0.1".parse().unwrap(),
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
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .expect("Content-Length");
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
    registry.add_output(device.wled);
    let settings = AdapterSettings::new(
        Wled::KIND,
        &WledSettings {
            address: "127.0.0.1".parse().unwrap(),
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
        address: "127.0.0.1".parse().unwrap(),
        leds: one_each(),
    };
    let error = Wled::local(port, 4048).open(&settings).err().unwrap();
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
fn validation_rejects_non_unicast_addresses_and_led_totals() {
    let valid = WledSettings {
        address: "192.168.1.50".parse().unwrap(),
        leds: one_each(),
    };
    assert_eq!(Wled::default().validate(&valid), Ok(()));
    for address in [
        "0.0.0.0",
        "255.255.255.255",
        "224.0.0.1",
        "::",
        "ff02::1",
        "::ffff:224.0.0.1",
        "::ffff:255.255.255.255",
    ] {
        let settings = WledSettings {
            address: address.parse().unwrap(),
            ..valid.clone()
        };
        assert_eq!(
            settings.validate().unwrap_err().field,
            "output.address",
            "{address}"
        );
    }
    for leds in [
        LedCounts::default(),
        LedCounts {
            top: u16::MAX,
            left: 1,
            ..LedCounts::default()
        },
    ] {
        let settings = WledSettings {
            leds,
            ..valid.clone()
        };
        assert_eq!(settings.validate().unwrap_err().field, "output.leds");
    }
    let most = WledSettings {
        leds: LedCounts {
            top: u16::MAX,
            ..LedCounts::default()
        },
        ..valid
    };
    assert_eq!(most.validate(), Ok(()));
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
        json!({"address": "wled.local", "leds": {"top": 1}}),
        json!({"address": "10.0.0.5", "leds": {"top": 65536}}),
    ] {
        assert!(serde_json::from_value::<WledSettings>(invalid).is_err());
    }
}
