use super::*;
use crate::{native::Video, pixels};
use prysm_webos_protocol::{HEADER_LEN, Header, PixelFormat};
use std::net::TcpListener;

#[test]
fn receiver_stop_without_frames_ends_session_and_drops_capture() {
    struct Pending(std::sync::Arc<AtomicBool>);
    impl Drop for Pending {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    impl FrameCapture for Pending {
        fn capture(&mut self, _: &mut Vec<u8>, _: &mut Events) -> Result<Option<Header>> {
            Ok(None)
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = Config::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    );
    let (opened, ready) = std::sync::mpsc::channel();
    let receiver = std::thread::spawn(move || {
        let socket = crate::transport::tests::accept(&listener, 30);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        socket.get_ref().shutdown(std::net::Shutdown::Both).unwrap();
    });
    let stop = AtomicBool::new(false);
    let dropped = std::sync::Arc::new(AtomicBool::new(false));
    capture_loop(&config, &stop, |_| {
        opened.send(()).unwrap();
        Ok(Pending(dropped.clone()))
    })
    .unwrap();
    assert!(!stop.load(Ordering::Relaxed));
    assert!(dropped.load(Ordering::Relaxed));
    receiver.join().unwrap();
}

#[test]
fn native_nv12_reaches_socket_with_raw_or_lz4_payload() {
    for compression in [Compression::None, Compression::Lz4] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = Config {
            compression,
            ..Config::new(
                format!("http://{}", listener.local_addr().unwrap())
                    .parse()
                    .unwrap(),
            )
        };
        let receiver = std::thread::spawn(move || {
            crate::transport::tests::accept_resolution(&listener, 15, [16, 8])
        });
        let stop = AtomicBool::new(false);
        capture_loop(&config, &stop, |options| {
            assert_eq!(
                *options,
                CaptureOptions {
                    width: 16,
                    height: 8,
                    fps: 15
                }
            );
            Ok(|output: &mut Vec<u8>| {
                *output = vec![63; 16 * 8];
                output.extend([102, 240].repeat(16 * 8 / 4));
                stop.store(true, Ordering::Relaxed);
                Ok(Some(pixels::header(16, 8, PixelFormat::Nv12)?))
            })
        })
        .unwrap();
        let mut socket = receiver.join().unwrap();
        socket
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let bytes = socket.read().unwrap().into_data();
        let header = Header::decode(bytes[..HEADER_LEN].try_into().unwrap()).unwrap();
        assert_eq!(header.format, PixelFormat::Nv12);
        assert_eq!(header.raw_len().unwrap(), 16 * 8 * 3 / 2);
        assert_eq!(
            header.codec == prysm_webos_protocol::Codec::Lz4,
            compression == Compression::Lz4
        );
        let payload = bytes[HEADER_LEN..].to_vec();
        assert_eq!(payload.len(), header.payload_len);
        let raw = if header.codec == prysm_webos_protocol::Codec::Lz4 {
            let mut decoded = vec![0; header.raw_len().unwrap()];
            prysm_webos_protocol::decompress(&payload, &mut decoded).unwrap();
            decoded
        } else {
            payload
        };
        assert_eq!(&raw[..16 * 8], &[63; 16 * 8]);
        assert_eq!(&raw[16 * 8..], [102, 240].repeat(16 * 8 / 4));
    }
}

#[test]
fn startup_retries_without_emitting_stale_frames() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = Config::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    );
    let receiver = std::thread::spawn(move || crate::transport::tests::accept(&listener, 15));
    let stop = AtomicBool::new(false);
    let mut attempts = 0;
    let start = Instant::now();
    capture_loop(&config, &stop, |options| -> Result<Video> {
        assert_eq!(options.fps, 15);
        attempts += 1;
        if attempts == 2 {
            stop.store(true, Ordering::Relaxed);
        }
        anyhow::bail!("driver not ready")
    })
    .unwrap();
    assert_eq!(attempts, 2);
    assert!(start.elapsed() >= Duration::from_secs(1));
    let mut socket = receiver.join().unwrap();
    socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    assert!(socket.read().is_err());
}

#[test]
fn reconnect_applies_new_rate_and_resolution_before_capture() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = Config::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    );
    let receiver = std::thread::spawn(move || {
        for (rate, resolution) in [(15, [2, 2]), (20, [4, 2])] {
            let mut socket =
                crate::transport::tests::accept_resolution(&listener, rate, resolution);
            let bytes = socket.read().unwrap().into_data();
            let header = Header::decode(bytes[..HEADER_LEN].try_into().unwrap()).unwrap();
            assert_eq!([header.width, header.height], resolution);
            assert_eq!(bytes.len(), HEADER_LEN + header.payload_len);
            socket.get_ref().shutdown(std::net::Shutdown::Both).unwrap();
        }
    });
    let stop = AtomicBool::new(false);
    let mut settings = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    for _ in 0..2 {
        capture_loop(&config, &stop, |options| {
            settings.push(*options);
            let options = *options;
            let stop = &stop;
            Ok(move |output: &mut Vec<u8>| {
                assert!(Instant::now() < deadline, "sender did not reconnect");
                let header = pixels::header(options.width, options.height, PixelFormat::Nv12)?;
                output.resize(header.raw_len()?, 128);
                if options.width == 4 {
                    stop.store(true, Ordering::Relaxed);
                }
                Ok(Some(header))
            })
        })
        .unwrap();
    }
    assert_eq!(
        settings,
        [
            CaptureOptions {
                width: 2,
                height: 2,
                fps: 15
            },
            CaptureOptions {
                width: 4,
                height: 2,
                fps: 20
            }
        ]
    );
    receiver.join().unwrap();
}

#[test]
fn failed_connection_opens_no_capture_resources() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = Config::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    );
    drop(listener);
    assert!(
        capture_loop(&config, &AtomicBool::new(false), |_| -> Result<Video> {
            panic!("failed connection opened capture")
        })
        .is_err()
    );
}

#[test]
fn idle_connection_opens_no_native_resources_and_remains_interruptible() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = Config::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    );
    let (release, hold) = std::sync::mpsc::channel();
    let receiver = std::thread::spawn(move || {
        let mut socket = crate::transport::tests::hello(&listener);
        socket
            .send(tokio_tungstenite::tungstenite::Message::binary(
                ControlMessage::Idle.encode().unwrap(),
            ))
            .unwrap();
        hold.recv_timeout(Duration::from_secs(2)).unwrap();
    });
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            stop.store(true, Ordering::Relaxed);
        });
        capture_loop(&config, &stop, |_| -> Result<Video> {
            panic!("idle opened capture")
        })
        .unwrap();
    });
    release.send(()).unwrap();
    receiver.join().unwrap();
}

#[test]
fn losing_video_while_idle_closes_the_receiver_connection() {
    use crate::control::Control;
    use prysm_webos_protocol::ControlMessage;
    use std::{
        fs::File,
        io::{Read, Write},
    };
    use std::{net::TcpListener, os::fd::OwnedFd, time::Duration};

    let (input, mut updates) = std::io::pipe().unwrap();
    let mut control = Control::new(File::from(OwnedFd::from(input))).unwrap();
    let stop = AtomicBool::new(false);
    updates.write_all(b"1").unwrap();
    assert!(control.wait_for_video(&stop).unwrap());

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = Config::new(
        format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
    );
    let peer = std::thread::spawn(move || {
        let mut socket = crate::transport::tests::hello(&listener);
        socket
            .send(tokio_tungstenite::tungstenite::Message::binary(
                ControlMessage::Idle.encode().unwrap(),
            ))
            .unwrap();
        socket
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        // The receiver never asks for capture. Losing video alone must release the slot.
        assert_eq!(socket.get_mut().read(&mut [0]).unwrap(), 0);
    });
    let mut session = Session::<Video>::connect(&config).unwrap();
    let mut first_poll = true;
    assert!(
        !session
            .wait_for_capture(&config, || {
                let ready = control.video_ready(&stop)?;
                if std::mem::take(&mut first_poll) {
                    assert!(ready);
                    updates.write_all(b"0")?;
                }
                Ok(ready)
            })
            .unwrap()
    );
    drop(session);
    peer.join().unwrap();
}
