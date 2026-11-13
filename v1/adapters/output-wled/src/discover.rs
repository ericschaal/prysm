//! Finding WLED devices on the local network, on request.
//!
//! One multicast DNS question for `_wled._tcp.local` (RFC 6762) sent from an
//! ephemeral port: a one-shot "legacy unicast" query, which devices answer
//! straight to the asking socket, so nothing binds port 5353 or joins a group.
//! Answers are collected for [`WINDOW`]; the address is the one each answer
//! came from (IPv4). Then one `GET /json/info` per device, in parallel and
//! bounded by [`INFO`], gives its name and LED count. Nothing else is sent.
//!
//! Only devices the multicast question reaches answer: the same network
//! segment, and in a container only with host networking.
use crate::http;
use prysm_ports::Candidate;
use serde::{Deserialize, Serialize};
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// How long answers are collected.
pub const WINDOW: Duration = Duration::from_millis(1200);
/// Bounds the connect and each read of one `/json/info`.
pub const INFO: Duration = Duration::from_millis(350);
const SERVICE: [&str; 3] = ["_wled", "_tcp", "local"];
/// `_wled._tcp.local`, type PTR, class IN.
const QUESTION: &[u8] = b"\x05_wled\x04_tcp\x05local\x00\x00\x0c\x00\x01";
const MDNS: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(224, 0, 0, 251)), 5353);
const PTR: u16 = 12;
/// More answers than this are ignored.
const MAX_DEVICES: usize = 16;

/// Where the question goes and how long each step waits. Tests replace the
/// multicast group and WLED's HTTP port with loopback fixtures.
pub struct Browse {
    pub group: SocketAddr,
    pub http_port: u16,
    pub window: Duration,
    pub info: Duration,
}

impl Default for Browse {
    fn default() -> Self {
        Self {
            group: MDNS,
            http_port: 80,
            window: WINDOW,
            info: INFO,
        }
    }
}

impl Browse {
    /// The devices that answered, in order of answer. Nothing when the
    /// question cannot be sent (no network).
    pub fn candidates(&self) -> Vec<Candidate> {
        let found = match self.answers() {
            Ok(found) => found,
            Err(error) => {
                tracing::debug!(%error, "WLED discovery could not ask the network");
                return Vec::new();
            }
        };
        thread::scope(|scope| {
            let described: Vec<_> = found
                .into_iter()
                .map(|(ip, instance)| scope.spawn(move || self.describe(ip, instance)))
                .collect();
            described
                .into_iter()
                .filter_map(|thread| thread.join().ok())
                .collect()
        })
    }

    /// Each answering device's address and service instance name.
    fn answers(&self) -> io::Result<Vec<(Ipv4Addr, String)>> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        // RFC 6762 asks for TTL 255 on the local link.
        socket.set_multicast_ttl_v4(255)?;
        socket.send_to(&query(), self.group)?;
        let deadline = Instant::now() + self.window;
        let mut found: Vec<(Ipv4Addr, String)> = Vec::new();
        let mut packet = [0; 9000];
        while found.len() < MAX_DEVICES {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            socket.set_read_timeout(Some(left))?;
            let (size, from) = match socket.recv_from(&mut packet) {
                Ok(received) => received,
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    break;
                }
                // A stray ICMP error or signal; keep listening.
                Err(_) => continue,
            };
            let SocketAddr::V4(from) = from else {
                continue;
            };
            if let Some(instance) = instance(&packet[..size])
                && !found.iter().any(|(ip, _)| ip == from.ip())
            {
                found.push((*from.ip(), instance));
            }
        }
        Ok(found)
    }

    /// The device's name and LED count from `/json/info`, else its instance name.
    fn describe(&self, ip: Ipv4Addr, instance: String) -> Candidate {
        #[derive(Deserialize)]
        struct Info {
            #[serde(default)]
            name: String,
            leds: Option<Leds>,
        }
        #[derive(Deserialize)]
        struct Leds {
            count: u32,
        }
        #[derive(Serialize)]
        struct Fields<'a> {
            address: &'a str,
        }
        let address = if self.http_port == 80 {
            ip.to_string()
        } else {
            format!("{ip}:{}", self.http_port)
        };
        let info = http::get_json(
            SocketAddr::from((ip, self.http_port)),
            "/json/info",
            self.info,
        )
        .ok()
        .and_then(|info| serde_json::from_value::<Info>(info).ok());
        let mut detail = address.clone();
        let mut label = instance;
        if let Some(info) = info {
            if !info.name.trim().is_empty() {
                label = info.name;
            }
            if let Some(Leds { count }) = info.leds {
                detail = format!("{address}, {count} LEDs");
            }
        }
        Candidate::new(label, &Fields { address: &address }).detail(detail)
    }
}

/// A standard query with one question: `_wled._tcp.local` PTR, class IN.
fn query() -> Vec<u8> {
    // Responders echo the ID to a legacy unicast query; any value will do.
    let id = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(1, |since| since.subsec_nanos().to_be_bytes()[3].into());
    let mut packet = Vec::with_capacity(12 + QUESTION.len());
    packet.extend_from_slice(&u16::to_be_bytes(id));
    packet.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    packet.extend_from_slice(QUESTION);
    packet
}

/// The service instance a response names for `_wled._tcp.local`, such as
/// `wled-living` for `wled-living._wled._tcp.local`.
fn instance(packet: &[u8]) -> Option<String> {
    let header = packet.get(..12)?;
    if header[2] & 0x80 == 0 {
        return None; // A question, such as our own or another querier's.
    }
    let count = |at: usize| usize::from(u16::from_be_bytes([header[at], header[at + 1]]));
    let mut at = 12;
    for _ in 0..count(4) {
        at = read_name(packet, at)?.1 + 4;
    }
    for _ in 0..count(6) + count(8) + count(10) {
        let (owner, fixed) = read_name(packet, at)?;
        let head = packet.get(fixed..fixed + 10)?;
        let kind = u16::from_be_bytes([head[0], head[1]]);
        let length = usize::from(u16::from_be_bytes([head[8], head[9]]));
        let data = fixed + 10;
        packet.get(data..data + length)?;
        if kind == PTR && is_service(&owner) {
            let (target, _) = read_name(packet, data)?;
            if let [instance, rest @ ..] = target.as_slice()
                && is_service(rest)
            {
                return Some(instance.clone());
            }
        }
        at = data + length;
    }
    None
}

fn is_service(labels: &[String]) -> bool {
    labels.len() == SERVICE.len()
        && labels
            .iter()
            .zip(SERVICE)
            .all(|(label, expected)| label.eq_ignore_ascii_case(expected))
}

/// The labels of the name at `at`, following compression pointers, and the
/// offset just after the name where it starts.
fn read_name(packet: &[u8], mut at: usize) -> Option<(Vec<String>, usize)> {
    let mut labels = Vec::new();
    let mut end = None;
    // Bounds pointer loops: a name has at most 127 labels.
    for _ in 0..128 {
        let length = usize::from(*packet.get(at)?);
        match length {
            0 => return Some((labels, end.unwrap_or(at + 1))),
            1..=63 => {
                let label = packet.get(at + 1..at + 1 + length)?;
                labels.push(String::from_utf8_lossy(label).into_owned());
                at += 1 + length;
            }
            0xC0.. => {
                end.get_or_insert(at + 2);
                at = (length & 0x3F) << 8 | usize::from(*packet.get(at + 1)?);
            }
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// A response as an ESP mDNS responder sends it: the PTR answer, with the
    /// instance name compressed against the question, plus an A record.
    fn response(instance: &str) -> Vec<u8> {
        let mut packet = vec![0, 1, 0x84, 0, 0, 1, 0, 1, 0, 0, 0, 1];
        packet.extend_from_slice(QUESTION); // its name at offset 12
        // Answer: owner -> pointer to 12, PTR, IN, TTL 120.
        packet.extend_from_slice(&[0xC0, 12, 0, 12, 0, 1, 0, 0, 0, 120]);
        let length = u8::try_from(instance.len()).unwrap();
        packet.extend_from_slice(&[0, length + 3, length]);
        packet.extend_from_slice(instance.as_bytes());
        packet.extend_from_slice(&[0xC0, 12]);
        // Additional: an A record for some name, which is skipped.
        packet.extend_from_slice(&[4, b'w', b'l', b'e', b'd', 0xC0, 23]);
        packet.extend_from_slice(&[0, 1, 0, 1, 0, 0, 0, 120, 0, 4, 192, 168, 1, 40]);
        packet
    }

    #[test]
    fn the_query_asks_one_ptr_question() {
        let packet = query();
        assert_eq!(&packet[2..12], &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            &packet[12..],
            b"\x05_wled\x04_tcp\x05local\x00\x00\x0c\x00\x01"
        );
        assert_eq!(packet[12..].len(), QUESTION.len());
        // A question is not mistaken for an answer.
        assert_eq!(instance(&packet), None);
    }

    #[test]
    fn a_response_names_its_instance() {
        assert_eq!(
            instance(&response("wled-living")).as_deref(),
            Some("wled-living")
        );
    }

    #[test]
    fn other_services_and_malformed_packets_are_ignored() {
        let mut other = response("printer");
        other[14] = b'x'; // `_wled` -> `_xled` in the name the answer points at
        assert_eq!(instance(&other), None);
        let whole = response("wled-living");
        // The answer ends before the 21-byte A record.
        let answered = whole.len() - 21;
        for cut in 0..whole.len() {
            assert_eq!(instance(&whole[..cut]).is_some(), cut >= answered, "{cut}");
        }
        // A pointer to itself.
        let mut looped = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        looped.extend_from_slice(&[0xC0, 12, 0, 12, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(instance(&looped), None);
    }

    /// A responder on loopback standing in for the multicast group: it reads
    /// the question and answers from 127.0.0.1 with each instance name.
    fn responder(instances: &'static [&'static str]) -> (SocketAddr, mpsc::Receiver<Vec<u8>>) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let (questions, asked) = mpsc::channel();
        let group = socket.local_addr().unwrap();
        thread::spawn(move || {
            let mut packet = [0; 512];
            let (size, from) = socket.recv_from(&mut packet).unwrap();
            questions.send(packet[..size].to_vec()).unwrap();
            for instance in instances {
                socket.send_to(&response(instance), from).unwrap();
            }
        });
        (group, asked)
    }

    /// An HTTP listener that answers one request with `reply`; the request head.
    fn device(reply: &'static str) -> (u16, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (heads, requested) = mpsc::channel();
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") && socket.read(&mut byte).unwrap() == 1 {
                head.push(byte[0]);
            }
            heads.send(String::from_utf8(head).unwrap()).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{reply}",
                reply.len()
            );
            socket.write_all(response.as_bytes()).unwrap();
        });
        (port, requested)
    }

    fn browse(group: SocketAddr, http_port: u16) -> Browse {
        Browse {
            group,
            http_port,
            window: Duration::from_millis(300),
            info: INFO,
        }
    }

    #[test]
    fn found_devices_are_named_from_their_info() {
        let (group, asked) = responder(&["wled-living", "wled-living"]);
        let (port, requested) =
            device(r#"{"ver":"0.15.0","name":"Living room","leds":{"count":120,"fps":42}}"#);
        let started = Instant::now();
        let found = browse(group, port).candidates();
        assert!(started.elapsed() < Duration::from_secs(2), "bounded");
        assert_eq!(asked.recv().unwrap()[12..], *QUESTION);
        let head = requested.recv().unwrap();
        assert!(head.starts_with("GET /json/info HTTP/1.1\r\n"), "{head}");
        let address = format!("127.0.0.1:{port}");
        // The repeated answer is one device.
        assert_eq!(
            found,
            [
                Candidate::new("Living room", &serde_json::json!({ "address": address }))
                    .detail(format!("{address}, 120 LEDs"))
            ]
        );
    }

    #[test]
    fn a_device_without_info_keeps_its_instance_name() {
        let (group, _asked) = responder(&["wled-desk"]);
        // Nothing listens on the HTTP port: the connection is refused.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let found = browse(group, port).candidates();
        let address = format!("127.0.0.1:{port}");
        assert_eq!(
            found,
            [
                Candidate::new("wled-desk", &serde_json::json!({ "address": address }))
                    .detail(address)
            ]
        );
    }

    #[test]
    fn nobody_answering_finds_nothing_within_the_window() {
        // A bound socket that never answers.
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let started = Instant::now();
        let found = browse(silent.local_addr().unwrap(), 80).candidates();
        assert!(found.is_empty());
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(300) && waited < Duration::from_secs(1));
    }
}
