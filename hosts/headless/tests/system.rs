//! The built `prysm-headless` as a whole system on loopback: a TV stand-in
//! streams frames over the capture stream (`ws://`, and `wss://` through a
//! TLS-terminating proxy) into a fake WLED (DDP on UDP 4048 and the JSON
//! API), and settings files persist across restarts.
//!
//! The binary sends DDP to port 4048, which it does not let settings change,
//! so the fake WLED binds 127.0.0.1:4048 and the tests that use it take
//! [`WLED`] one at a time. Nothing leaves 127.0.0.1.
#[path = "../../common/tests/launch/mod.rs"]
mod launch;

use launch::{Host, Reply, TIMEOUT, get, null_settings, put_target, request};
use prysm_capture_remote::RemoteSender;
use prysm_capture_remote::testkit::{in_test_ca_process, tls_proxy};
use prysm_core::{Frame, PixelFormat};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender as Channel, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const BINARY: &str = env!("CARGO_BIN_EXE_prysm-headless");

/// Taken by every test that binds the fake WLED's UDP 4048.
static WLED: Mutex<()> = Mutex::new(());

fn wled_lock() -> MutexGuard<'static, ()> {
    WLED.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 12 LEDs: one DDP packet per frame.
const LEDS: [usize; 4] = [4, 2, 4, 2];
const RED: [u8; 3] = [220, 16, 16];
const BLUE: [u8; 3] = [16, 16, 220];

fn remote_capture() -> Value {
    json!({ "kind": "remote", "width": 32, "height": 18, "fps": 30, "encoding": "raw" })
}

fn write_settings(dir: &Path, settings: &Value) -> String {
    let path = dir.join("settings.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    path.to_str().unwrap().to_owned()
}

fn start(settings: &str) -> (Host, u16) {
    let mut host = Host::spawn(BINARY, &[settings, "--listen", "127.0.0.1:0"]);
    let port = host.portal_port();
    (host, port)
}

fn state(port: u16) -> Value {
    let (status, body) = get(port, "/api/v1/state");
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

/// The state once `ready` holds; panics with the last state after [`TIMEOUT`].
fn wait_for_state(port: u16, what: &str, ready: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let state = state(port);
        if ready(&state) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "never {what}; last state {state:#}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn session(state: &Value) -> &str {
    state["session"]["state"].as_str().unwrap()
}

fn put_json(port: u16, path: &str, body: &Value) -> Reply {
    request(
        port,
        "PUT",
        path,
        &[("Content-Type", "application/json")],
        &body.to_string(),
    )
}

/// `PUT /api/v1/settings` replacing the settings `state` shows with `settings`.
fn save_settings(port: u16, state: &Value, settings: &Value) -> Reply {
    put_json(
        port,
        "/api/v1/settings",
        &json!({ "expected": state["revision"], "settings": settings }),
    )
}

/// What the fake WLED's HTTP API was asked.
#[derive(Debug)]
enum Call {
    /// `POST /json/state` with this body.
    State(Value),
    /// `GET /json/si`: the output checking whose colours it shows.
    Check,
}

/// A WLED on loopback: the JSON API on an ephemeral port, DDP on 4048.
struct FakeWled {
    http_port: u16,
    ddp: UdpSocket,
    calls: Receiver<Call>,
}

impl FakeWled {
    /// `fail_state`: every `POST /json/state` answers 500, as a WLED busy
    /// with an update does.
    fn start(fail_state: bool) -> Self {
        let ddp = UdpSocket::bind("127.0.0.1:4048").unwrap_or_else(|error| {
            panic!("the fake WLED needs 127.0.0.1:4048, which is taken: {error}")
        });
        ddp.set_read_timeout(Some(TIMEOUT)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let http_port = listener.local_addr().unwrap().port();
        let (send, calls) = channel();
        thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                if !answer(socket, &send, fail_state) {
                    return;
                }
            }
        });
        Self {
            http_port,
            ddp,
            calls,
        }
    }

    fn address(&self) -> String {
        format!("127.0.0.1:{}", self.http_port)
    }

    /// The next `POST /json/state` body, skipping state checks.
    fn next_state(&self) -> Value {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.calls.recv_timeout(left) {
                Ok(Call::State(body)) => return body,
                Ok(Call::Check) => {}
                Err(error) => panic!("no POST /json/state: {error}"),
            }
        }
    }

    /// Skip state bodies until `wanted` arrives; what came before it.
    fn expect_state(&self, wanted: &Value) -> Vec<Value> {
        let mut before = Vec::new();
        loop {
            let body = self.next_state();
            if &body == wanted {
                return before;
            }
            before.push(body);
        }
    }

    /// The LED colours of the next DDP packet.
    fn next_colours(&self) -> Vec<[u8; 3]> {
        let mut packet = [0u8; 1500];
        let (count, _) = self
            .ddp
            .recv_from(&mut packet)
            .unwrap_or_else(|error| panic!("no DDP packet: {error}"));
        assert!(count >= 10, "short DDP packet");
        assert_eq!(packet[0] & 0xc0, 0x40, "DDP version 1");
        let length = usize::from(u16::from_be_bytes([packet[8], packet[9]]));
        assert_eq!(count, 10 + length, "DDP length field");
        let (leds, rest) = packet[10..count].as_chunks::<3>();
        assert!(rest.is_empty(), "whole RGB triplets");
        leds.to_vec()
    }

    /// Wait for a packet whose every LED is clearly `colour` (its strongest
    /// channel well above the others), as smoothing settles.
    fn expect_colour(&self, colour: [u8; 3]) {
        let strongest = (0..3).max_by_key(|&i| colour[i]).unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let leds = self.next_colours();
            assert_eq!(leds.len(), LEDS.iter().sum::<usize>(), "one colour per LED");
            let all = leds.iter().all(|led| {
                (0..3).all(|i| i == strongest || u16::from(led[i]) * 2 < u16::from(led[strongest]))
                    && led[strongest] > 50
            });
            if all {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the LEDs never showed {colour:?}: {leds:?}"
            );
        }
    }
}

/// Serve one HTTP request on `socket`; `false` once the test has ended.
fn answer(mut socket: TcpStream, calls: &Channel<Call>, fail_state: bool) -> bool {
    let _ = socket.set_read_timeout(Some(TIMEOUT));
    let mut read = Vec::new();
    let mut chunk = [0; 4096];
    let (head, body) = loop {
        if let Some(end) = read.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&read[..end]).into_owned();
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if read.len() >= end + 4 + length {
                break (head, read[end + 4..end + 4 + length].to_vec());
            }
        }
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return true,
            Ok(n) => read.extend_from_slice(&chunk[..n]),
        }
    };
    let (status, reply, call) = if head.starts_with("POST /json/state ") {
        let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if fail_state {
            (
                "500 Internal Server Error",
                "{}".to_owned(),
                Call::State(body),
            )
        } else {
            (
                "200 OK",
                r#"{"success":true}"#.to_owned(),
                Call::State(body),
            )
        }
    } else if head.starts_with("GET /json/si ") {
        let si = json!({
            "state": { "on": true, "lor": 0 },
            "info": { "live": true, "lm": "DDP", "lip": "127.0.0.1" },
        });
        ("200 OK", si.to_string(), Call::Check)
    } else {
        let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return true;
    };
    let _ = write!(
        socket,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
    calls.send(call).is_ok()
}

/// A stand-in for the TV's slim worker on a thread: connects (again every
/// 50 ms while the portal answers 503), refuses its first `refuse`
/// connections, then streams flat `colour` frames at the requested size and
/// rate until dropped.
struct TvSender {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    events: Receiver<&'static str>,
}

impl TvSender {
    fn start(url: &str, address: SocketAddr, refuse: u32, colour: [u8; 3]) -> Self {
        let url = url.parse().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let (events, received) = channel();
        let thread = thread::spawn(move || {
            let mut refusals = refuse;
            while !stopping.load(Ordering::Relaxed) {
                let Ok(mut sender) = RemoteSender::connect(&url, address) else {
                    thread::sleep(Duration::from_millis(50));
                    continue;
                };
                if refusals > 0 {
                    refusals -= 1;
                    sender.refuse("the TV's capture is busy");
                    let _ = events.send("refused");
                    continue;
                }
                let request = sender.request();
                let _ = events.send("connected");
                let pixels = colour.repeat((request.width * request.height) as usize);
                let frame =
                    Frame::new(pixels, request.width, request.height, PixelFormat::RGB24).unwrap();
                let interval = Duration::from_secs(1) / request.fps;
                while !stopping.load(Ordering::Relaxed) && sender.deliver(&frame).is_ok() {
                    thread::sleep(interval);
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
            events: received,
        }
    }

    /// Plain `ws://` straight to the portal.
    fn plain(port: u16, refuse: u32, colour: [u8; 3]) -> Self {
        Self::start(
            &format!("ws://127.0.0.1:{port}/api/v1/capture/stream"),
            SocketAddr::from(([127, 0, 0, 1], port)),
            refuse,
            colour,
        )
    }

    fn expect(&self, event: &str) {
        let got = self
            .events
            .recv_timeout(TIMEOUT)
            .unwrap_or_else(|error| panic!("no `{event}` from the sender: {error}"));
        assert_eq!(got, event);
    }
}

impl Drop for TvSender {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn wled_settings(wled: &FakeWled) -> Value {
    json!({
        "capture": remote_capture(),
        "output": {
            "kind": "wled",
            "address": wled.address(),
            "leds": { "top": LEDS[0], "right": LEDS[1], "bottom": LEDS[2], "left": LEDS[3] },
        },
    })
}

const LIVE: &str = r#"{"on":true,"lor":0,"live":true,"tt":0}"#;
const OFF: &str = r#"{"on":false,"lor":2,"tt":0}"#;
const NATIVE: &str = r#"{"on":true,"lor":2,"tt":0}"#;

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// Frames from the TV reach the LEDs; on SIGTERM WLED is released and off.
#[test]
fn a_tv_stream_lights_wled_and_sigterm_turns_it_off() {
    let _wled = wled_lock();
    let wled = FakeWled::start(false);
    let dir = tempfile::tempdir().unwrap();
    let (host, port) = start(&write_settings(dir.path(), &wled_settings(&wled)));
    let sender = TvSender::plain(port, 0, RED);

    assert_eq!(put_target(port, "processed", None).status, 204);
    sender.expect("connected");
    // The output opens at the first frame: out of a realtime session, then live.
    assert_eq!(wled.next_state(), json!({ "live": false }));
    assert_eq!(wled.next_state(), body(LIVE));
    wled.expect_colour(RED);
    wait_for_state(port, "running", |state| session(state) == "running");

    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
    assert_eq!(wled.next_state(), json!({ "live": false }));
    assert_eq!(wled.next_state(), body(OFF));
}

/// When the TV stops sending, the lights go back to WLED's own state after
/// the grace period, and return to live colours when it sends again.
#[test]
fn a_dropped_sender_releases_wled_to_its_own_state() {
    let _wled = wled_lock();
    let wled = FakeWled::start(false);
    let dir = tempfile::tempdir().unwrap();
    let (host, port) = start(&write_settings(dir.path(), &wled_settings(&wled)));
    let sender = TvSender::plain(port, 0, RED);
    assert_eq!(put_target(port, "processed", None).status, 204);
    sender.expect("connected");
    wled.expect_state(&body(LIVE));
    wled.expect_colour(RED);

    drop(sender);
    // After the session's three-second grace without frames.
    wled.expect_state(&body(NATIVE));
    let state = state(port);
    assert_eq!(session(&state), "running", "{state:#}");
    assert_eq!(state["failure"], Value::Null);

    let sender = TvSender::plain(port, 0, BLUE);
    sender.expect("connected");
    wled.expect_state(&body(LIVE));
    wled.expect_colour(BLUE);
    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
}

/// The same stream over `wss://` through a TLS-terminating proxy, as a TV
/// reaches a Prysm behind HTTPS. The sender trusts only the system's
/// authorities, so the test runs itself again with the test CA.
#[test]
fn a_tv_stream_through_a_tls_proxy_lights_wled() {
    // Held by the parent while its child runs the test with the test CA.
    let _wled = wled_lock();
    if !in_test_ca_process("a_tv_stream_through_a_tls_proxy_lights_wled") {
        return;
    }
    let wled = FakeWled::start(false);
    let dir = tempfile::tempdir().unwrap();
    let (host, port) = start(&write_settings(dir.path(), &wled_settings(&wled)));
    let proxy = tls_proxy(SocketAddr::from(([127, 0, 0, 1], port)));
    let sender = TvSender::start(
        &format!("wss://localhost:{}/api/v1/capture/stream", proxy.port()),
        proxy,
        0,
        BLUE,
    );
    assert_eq!(put_target(port, "processed", None).status, 204);
    sender.expect("connected");
    wled.expect_state(&body(LIVE));
    wled.expect_colour(BLUE);
    drop(sender);
    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
}

/// WLED refusing the switch to live mode is a typed output failure in the
/// state, and lighting stops.
#[test]
fn a_wled_error_is_a_typed_output_failure() {
    let _wled = wled_lock();
    let wled = FakeWled::start(true);
    let dir = tempfile::tempdir().unwrap();
    let (host, port) = start(&write_settings(dir.path(), &wled_settings(&wled)));
    let sender = TvSender::plain(port, 0, RED);
    assert_eq!(put_target(port, "processed", None).status, 204);
    sender.expect("connected");

    let state = wait_for_state(port, "failed", |state| !state["failure"].is_null());
    let failure = &state["failure"];
    assert_eq!(failure["kind"], "adapter", "{state:#}");
    assert_eq!(failure["section"], "output", "{state:#}");
    let message = failure["message"].as_str().unwrap();
    assert!(message.contains("did not switch to live mode"), "{message}");
    assert!(message.contains(&wled.address()), "{message}");
    assert_eq!(state["target"], "processed");
    assert_eq!(session(&state), "stopped");
    drop(sender);
    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
}

/// A start from WLED's own effects that failed leaves the session `native`
/// with the target `processed`; a settings save then retries the start
/// instead of clearing the failure and leaving the card "starting" (the
/// engine fix in ef7c4be, which the portal's null output cannot reach).
///
/// The start fails as it opens the source: a file source with no FFmpeg on
/// `PATH` (the host runs with an empty one). The save switches to the TV.
#[test]
fn a_save_after_a_failed_start_from_effects_retries() {
    let _wled = wled_lock();
    let wled = FakeWled::start(false);
    let dir = tempfile::tempdir().unwrap();
    let mut settings = wled_settings(&wled);
    settings["capture"] = json!({ "kind": "file", "path": "/nonexistent/clip.mkv" });
    let path = write_settings(dir.path(), &settings);
    let mut host = Host::spawn(
        "/usr/bin/env",
        &["PATH=", BINARY, &path, "--listen", "127.0.0.1:0"],
    );
    let port = host.portal_port();

    assert_eq!(put_target(port, "device_native", None).status, 204);
    wled.expect_state(&body(NATIVE));
    wait_for_state(port, "native", |state| session(state) == "native");

    put_target(port, "processed", None);
    let failed = wait_for_state(port, "failed from effects", |state| {
        !state["failure"].is_null()
    });
    assert_eq!(failed["target"], "processed", "{failed:#}");
    assert_eq!(session(&failed), "native", "{failed:#}");
    assert_eq!(failed["failure"]["section"], "capture", "{failed:#}");

    // The save retries the start, now with the TV streaming.
    let sender = TvSender::plain(port, 0, RED);
    let mut changed = failed["settings"].clone();
    changed["capture"] = remote_capture();
    let saved = save_settings(port, &failed, &changed);
    assert_eq!(saved.status, 200, "{}", saved.body);
    let state = wait_for_state(port, "running, or stuck starting", |state| {
        session(state) == "running" || state["failure"].is_null()
    });
    assert_eq!(state["failure"], Value::Null, "{state:#}");
    let state = wait_for_state(port, "running", |state| session(state) == "running");
    assert_eq!(state["target"], "processed");
    sender.expect("connected");
    wled.expect_state(&body(LIVE));
    wled.expect_colour(RED);
    drop(sender);
    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
}

/// Settings saved through the portal are in the file and survive a restart.
#[test]
fn saved_settings_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = null_settings(dir.path());
    let path = path.to_str().unwrap();
    let (host, port) = start(path);
    let before = state(port);
    let mut settings = before["settings"].clone();
    settings["capture"] = remote_capture();
    settings["processing"]["brightness_percent"] = json!(40);
    let saved = save_settings(port, &before, &settings);
    assert_eq!(saved.status, 200, "{}", saved.body);
    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");

    let file: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(file["capture"]["kind"], "remote", "{file:#}");
    assert_eq!(file["processing"]["brightness_percent"], 40, "{file:#}");

    let (host, port) = start(path);
    let after = state(port);
    assert_eq!(after["settings"], settings, "{after:#}");
    assert_eq!(after["target"], "off");
    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
}

/// A file that is not JSON stops the start, naming it, and is left as it
/// was; valid JSON with invalid settings starts idle with a typed failure,
/// so the portal can fix it.
#[test]
fn a_corrupt_or_invalid_settings_file_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let corrupt = dir.path().join("corrupt.json");
    std::fs::write(&corrupt, "{ not json").unwrap();
    let output = std::process::Command::new(BINARY)
        .args([corrupt.to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cannot parse settings file"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&corrupt).unwrap(), "{ not json");

    let invalid = write_settings(dir.path(), &json!({ "output": { "kind": "bogus" } }));
    let (host, port) = start(&invalid);
    let state = wait_for_state(port, "failed", |state| !state["failure"].is_null());
    assert_eq!(state["failure"]["kind"], "invalid", "{state:#}");
    assert_eq!(state["target"], "off");
    assert_eq!(
        std::fs::read_to_string(&invalid).unwrap(),
        r#"{"output":{"kind":"bogus"}}"#
    );
    // Fixed through the portal.
    let fixed = json!({ "output": { "kind": "null" } });
    let saved = save_settings(port, &state, &fixed);
    assert_eq!(saved.status, 200, "{}", saved.body);
    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&invalid).unwrap()).unwrap();
    assert_eq!(file["output"]["kind"], "null", "{file:#}");
}
