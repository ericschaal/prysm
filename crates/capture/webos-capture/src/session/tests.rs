use super::*;
use crate::pixels;
use clap::Parser;
use prysm_webos_protocol::{HEADER_LEN, Header, PixelFormat};
use std::{cell::Cell, io::Read, net::TcpListener};

#[test]
fn receiver_stop_without_frames_ends_the_session_and_drops_ui() {
    struct RecordingUi(std::sync::mpsc::Sender<()>, std::sync::Arc<AtomicBool>);
    impl Drop for RecordingUi {
        fn drop(&mut self) {
            self.1.store(true, Ordering::Relaxed);
        }
    }
    impl UiCapture for RecordingUi {
        fn configure(&mut self, _: &CaptureOptions) -> Result<()> {
            self.0.send(()).unwrap();
            Ok(())
        }
        fn capture(&mut self, _: u32, _: u32, _: &mut pixels::Bgra8Pixels) -> Result<bool> {
            Ok(false)
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cli = Cli::try_parse_from([
        "capture",
        &listener.local_addr().unwrap().to_string(),
        "--source",
        "ui",
    ])
    .unwrap();
    let (configured, ready) = std::sync::mpsc::channel();
    let receiver = std::thread::spawn(move || {
        let socket = crate::transport::tests::accept(&listener, 30);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        socket.shutdown(std::net::Shutdown::Both).unwrap();
    });
    let stop = AtomicBool::new(false);
    let dropped = std::sync::Arc::new(AtomicBool::new(false));
    capture_loop(
        &cli,
        &stop,
        |_| panic!("UI-only opened video"),
        Some(RecordingUi(configured, dropped.clone())),
    )
    .unwrap();
    assert!(!stop.load(Ordering::Relaxed));
    assert!(dropped.load(Ordering::Relaxed));
    receiver.join().unwrap();
}

#[test]
fn nv12_composition_reaches_the_socket_with_raw_or_lz4_payload() {
    for compression in ["none", "lz4"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cli = Cli::try_parse_from([
            "capture",
            &listener.local_addr().unwrap().to_string(),
            "--source",
            "ui",
            "--composition-format",
            "nv12",
            "--compression",
            compression,
        ])
        .unwrap();
        let receiver = std::thread::spawn(move || {
            crate::transport::tests::accept_resolution(&listener, 15, [16, 8])
        });
        let stop = AtomicBool::new(false);
        capture_loop(
            &cli,
            &stop,
            |_| panic!("UI-only opened video"),
            Some(|_, _, bgra: &mut pixels::Bgra8Pixels| {
                *bgra.bytes_mut() = [0, 0, 255, 255].repeat(16 * 8);
                stop.store(true, Ordering::Relaxed);
                Ok(())
            }),
        )
        .unwrap();
        let mut socket = receiver.join().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut bytes = [0; HEADER_LEN];
        socket.read_exact(&mut bytes).unwrap();
        let header = Header::decode(&bytes).unwrap();
        assert_eq!(header.format, PixelFormat::Nv12);
        assert_eq!(header.raw_len().unwrap(), 16 * 8 * 3 / 2);
        assert_eq!(
            header.codec == prysm_webos_protocol::Codec::Lz4,
            compression == "lz4"
        );
        let mut payload = vec![0; header.payload_len];
        socket.read_exact(&mut payload).unwrap();
        let raw = if header.codec == prysm_webos_protocol::Codec::Lz4 {
            let mut decoded = vec![0; header.raw_len().unwrap()];
            prysm_webos_protocol::decompress(&payload, &mut decoded).unwrap();
            decoded
        } else {
            payload
        };
        let mut rgb = Vec::new();
        pixels::to_rgb(header, &raw, &mut rgb).unwrap();
        assert!(
            rgb.chunks_exact(3)
                .all(|p| p[0] >= 250 && p[1] <= 4 && p[2] <= 4)
        );
    }
}

#[test]
fn startup_retries_while_ui_frames_continue() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cli =
        Cli::try_parse_from(["capture", &listener.local_addr().unwrap().to_string()]).unwrap();
    let receiver = std::thread::spawn(move || crate::transport::tests::accept(&listener, 15));
    let stop = AtomicBool::new(false);
    let mut attempts = 0;
    let ui_frames = Cell::new(0);
    let start = Instant::now();
    capture_loop(
        &cli,
        &stop,
        |options| {
            assert_eq!(options.fps, 15);
            attempts += 1;
            if attempts == 2 {
                assert!(ui_frames.get() > 0);
                stop.store(true, Ordering::Relaxed);
            }
            anyhow::bail!("driver not ready")
        },
        Some(|_, _, bgra: &mut pixels::Bgra8Pixels| {
            ui_frames.set(ui_frames.get() + 1);
            *bgra.bytes_mut() = [0, 0, 255, 255].repeat(4);
            Ok(())
        }),
    )
    .unwrap();
    assert_eq!(attempts, 2);
    assert!(start.elapsed() >= Duration::from_secs(1));
    let mut socket = receiver.join().unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut bytes = [0; HEADER_LEN];
    socket.read_exact(&mut bytes).unwrap();
    let header = Header::decode(&bytes).unwrap();
    assert_eq!(header.format, PixelFormat::Rgb);
    let mut pixels = vec![0; header.payload_len];
    socket.read_exact(&mut pixels).unwrap();
    assert_eq!(pixels, [255, 0, 0].repeat(4));
}

#[test]
fn reconnect_reconfigures_capture_before_acquiring_frames() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cli =
        Cli::try_parse_from(["capture", &listener.local_addr().unwrap().to_string()]).unwrap();
    let receiver = std::thread::spawn(move || {
        for rate in [15, 20] {
            let mut socket = crate::transport::tests::accept(&listener, rate);
            let mut header = [0; HEADER_LEN];
            socket.read_exact(&mut header).unwrap();
            let header = Header::decode(&header).unwrap();
            socket.read_exact(&mut vec![0; header.payload_len]).unwrap();
            socket.shutdown(std::net::Shutdown::Both).unwrap();
        }
    });
    let stop = AtomicBool::new(false);
    let mut rates = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    for _ in 0..2 {
        capture_loop(
            &cli,
            &stop,
            |options| {
                rates.push(options.fps);
                if options.fps == 20 {
                    stop.store(true, Ordering::Relaxed);
                }
                anyhow::bail!("test video unavailable")
            },
            Some(|_, _, bgra: &mut pixels::Bgra8Pixels| {
                assert!(Instant::now() < deadline, "sender did not reconnect");
                *bgra.bytes_mut() = [0, 0, 255, 255].repeat(4);
                Ok(())
            }),
        )
        .unwrap();
    }
    assert_eq!(rates, [15, 20]);
    receiver.join().unwrap();
}

#[test]
fn failed_connection_exits_before_ui_capture_or_composition() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cli = Cli::try_parse_from([
        "capture",
        &listener.local_addr().unwrap().to_string(),
        "--source",
        "ui",
    ])
    .unwrap();
    drop(listener);
    let stop = AtomicBool::new(false);
    let mut ui_frames = 0;
    assert!(
        capture_loop(
            &cli,
            &stop,
            |_| panic!("UI-only opened video"),
            Some(|_, _, _: &mut pixels::Bgra8Pixels| {
                ui_frames += 1;
                Ok(())
            }),
        )
        .is_err()
    );
    assert_eq!(ui_frames, 0);
}

#[test]
fn reconnect_applies_a_new_resolution_before_video_and_ui_capture() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cli =
        Cli::try_parse_from(["capture", &listener.local_addr().unwrap().to_string()]).unwrap();
    let receiver = std::thread::spawn(move || {
        for resolution in [[2, 2], [4, 2]] {
            let mut socket = crate::transport::tests::accept_resolution(&listener, 30, resolution);
            let mut header = [0; HEADER_LEN];
            socket.read_exact(&mut header).unwrap();
            let header = Header::decode(&header).unwrap();
            assert_eq!([header.width, header.height], resolution);
            socket.read_exact(&mut vec![0; header.payload_len]).unwrap();
            socket.shutdown(std::net::Shutdown::Both).unwrap();
        }
    });
    let stop = AtomicBool::new(false);
    let mut resolutions = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    for _ in 0..2 {
        capture_loop(
            &cli,
            &stop,
            |options| {
                resolutions.push([options.width, options.height]);
                assert_eq!(options.fps, 30);
                anyhow::bail!("test video unavailable")
            },
            Some(|width, height, bgra: &mut pixels::Bgra8Pixels| {
                assert!(Instant::now() < deadline, "sender did not reconnect");
                if width == 4 {
                    stop.store(true, Ordering::Relaxed);
                }
                *bgra.bytes_mut() = [0, 0, 255, 255].repeat((width * height) as usize);
                Ok(())
            }),
        )
        .unwrap();
    }
    assert_eq!(resolutions, [[2, 2], [4, 2]]);
    receiver.join().unwrap();
}

#[test]
fn idle_connection_opens_no_native_resources_and_remains_interruptible() {
    use std::io::Write;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cli =
        Cli::try_parse_from(["capture", &listener.local_addr().unwrap().to_string()]).unwrap();
    let (release, hold) = std::sync::mpsc::channel();
    let receiver = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut prefix = [0; 4];
        socket.read_exact(&mut prefix).unwrap();
        let mut bytes =
            vec![0; prysm_webos_protocol::ControlMessage::decode_length(prefix).unwrap()];
        socket.read_exact(&mut bytes).unwrap();
        socket
            .write_all(&prysm_webos_protocol::ControlMessage::Idle.encode().unwrap())
            .unwrap();
        hold.recv_timeout(Duration::from_secs(2)).unwrap();
    });
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            stop.store(true, Ordering::Relaxed);
        });
        capture_loop(
            &cli,
            &stop,
            |_| panic!("idle opened video"),
            Some(|_, _, _: &mut pixels::Bgra8Pixels| panic!("idle captured UI")),
        )
        .unwrap();
    });
    release.send(()).unwrap();
    receiver.join().unwrap();
}
