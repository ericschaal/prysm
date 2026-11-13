//! Launches a built host binary for the hosts' integration tests, which
//! include this file with `#[path]`. Every wait is bounded.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

pub const TIMEOUT: Duration = Duration::from_secs(10);

/// A settings file in `dir` with only the null output: no capture, no LEDs.
pub fn null_settings(dir: &Path) -> PathBuf {
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{ "output": { "kind": "null" } }"#).unwrap();
    path
}

pub struct Host {
    child: Child,
    lines: Receiver<String>,
    pub stderr: Vec<String>,
}

impl Host {
    /// Start `binary` with piped stdio, so without a terminal.
    pub fn spawn(binary: &str, args: &[&str]) -> Self {
        let mut child = Command::new(binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (send, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines,
            stderr: Vec::new(),
        }
    }

    /// The first stderr line (from now or earlier) containing `needle`.
    pub fn wait_for(&mut self, needle: &str) -> String {
        if let Some(line) = self.stderr.iter().find(|line| line.contains(needle)) {
            return line.clone();
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    self.stderr.push(line.clone());
                    if line.contains(needle) {
                        return line;
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    let _ = self.child.kill();
                    panic!("no `{needle}` on stderr; got {:#?}", self.stderr);
                }
            }
        }
    }

    /// The port from the `portal http://ADDR/` status line.
    pub fn portal_port(&mut self) -> u16 {
        let line = self.wait_for("portal http://");
        let address = line.split("http://").nth(1).unwrap();
        let address = &address[..address.find('/').unwrap()];
        address.rsplit(':').next().unwrap().parse().unwrap()
    }

    /// SIGTERM, then wait for exit; collects the rest of stderr.
    pub fn terminate(mut self) -> (ExitStatus, Vec<String>) {
        let killed = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(killed.success());
        let deadline = Instant::now() + TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("host did not exit after SIGTERM; stderr {:#?}", self.stderr);
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        while let Ok(line) = self.lines.recv_timeout(TIMEOUT) {
            self.stderr.push(line);
        }
        (status, std::mem::take(&mut self.stderr))
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A portal reply: status, raw header block and body.
pub struct Reply {
    pub status: u16,
    pub headers: String,
    pub body: String,
}

impl Reply {
    /// The value of header `name` (lowercase), if present.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }
}

/// One request to the loopback portal with `Host: 127.0.0.1:PORT`.
pub fn request(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    let extra: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    Reply {
        status: head[9..12].parse().unwrap(),
        headers: head.to_owned(),
        body: body.to_owned(),
    }
}

/// `GET path` on the loopback portal; returns the status code and body.
pub fn get(port: u16, path: &str) -> (u16, String) {
    let reply = request(port, "GET", path, &[], "");
    (reply.status, reply.body)
}

/// `PUT /api/v1/target` with an optional `Origin`.
pub fn put_target(port: u16, target: &str, origin: Option<&str>) -> Reply {
    let mut headers = vec![("Content-Type", "application/json")];
    headers.extend(origin.map(|origin| ("Origin", origin)));
    request(
        port,
        "PUT",
        "/api/v1/target",
        &headers,
        &format!(r#"{{"target":"{target}"}}"#),
    )
}

/// `--help` prints usage to stdout only; bad arguments exit 2 before touching settings.
pub fn check_arguments(binary: &str, name: &str) {
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.starts_with(&format!("usage: {name} [options] <settings.json>")));
    assert!(text.contains("--no-portal") && text.contains("--allow-origin"));
    assert!(!text.contains("token"));
    assert!(help.stderr.is_empty());

    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("unused.json");
    let settings = settings.to_str().unwrap();
    for args in [
        &[][..],
        &["--bogus"],
        &[settings, "--start", "--start"],
        &[settings, "--listen", "not-an-address"],
        &[settings, "other.json"],
        &[settings, "--token-file", "t"],
        &[
            settings,
            "--allow-origin",
            "*",
            "--allow-origin",
            "http://a",
        ],
    ] {
        let output = Command::new(binary).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8(output.stderr).unwrap().contains("usage:"));
        assert!(!Path::new(settings).exists());
    }
}

/// `--check-settings` validates without opening a device or the portal:
/// exit 0 and "settings OK", or exit 1 with the field and cause chain.
pub fn check_settings(binary: &str, name: &str) {
    let dir = tempfile::tempdir().unwrap();
    let check = |settings: &Path| {
        let output = Command::new(binary)
            .args(["--check-settings", settings.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!stderr.contains("portal"), "{stderr}");
        (output.status.code(), stderr)
    };

    let (code, stderr) = check(&null_settings(dir.path()));
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.ends_with(&format!("{name}: settings OK\n")),
        "{stderr}"
    );

    for (settings, message) in [
        (
            r#"{ "output": { "kind": "bogus" } }"#,
            "invalid output.kind: unknown kind `bogus`",
        ),
        (
            r#"{ "output": { "kind": "null", "leds": 1 } }"#,
            "cannot decode `null` settings: invalid type: integer `1`",
        ),
    ] {
        let path = dir.path().join("invalid.json");
        std::fs::write(&path, settings).unwrap();
        let (code, stderr) = check(&path);
        assert_eq!(code, Some(1), "{stderr}");
        assert!(stderr.contains(&format!("{name}: {message}")), "{stderr}");
    }

    let missing = dir.path().join("missing.json");
    let (code, stderr) = check(&missing);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("no settings saved yet"), "{stderr}");
    assert!(!missing.exists());
}
