use super::*;
use crate::pixels;
use clap::Parser;
use prysm_webos_protocol::{HEADER_LEN, Header, PixelFormat};
use std::{cell::Cell, io::Read, net::TcpListener};

#[test]
fn nv12_composition_reaches_the_socket_with_raw_or_lz4_payload() {
    for compression in ["none", "lz4"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cli = Cli::try_parse_from([
            "capture",
            &listener.local_addr().unwrap().to_string(),
            "--source",
            "ui",
            "--width",
            "16",
            "--height",
            "8",
            "--composition-format",
            "nv12",
            "--compression",
            compression,
        ])
        .unwrap();
        let receiver = std::thread::spawn(move || crate::transport::tests::accept(&listener, 15));
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
    let cli = Cli::try_parse_from([
        "capture",
        &listener.local_addr().unwrap().to_string(),
        "--width",
        "2",
        "--height",
        "2",
    ])
    .unwrap();
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
    let cli = Cli::try_parse_from([
        "capture",
        &listener.local_addr().unwrap().to_string(),
        "--width",
        "2",
        "--height",
        "2",
    ])
    .unwrap();
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
    assert_eq!(rates, [15, 20]);
    receiver.join().unwrap();
}

#[test]
fn network_backoff_skips_ui_capture_and_composition() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cli = Cli::try_parse_from([
        "capture",
        &listener.local_addr().unwrap().to_string(),
        "--source",
        "ui",
        "--width",
        "2",
        "--height",
        "2",
    ])
    .unwrap();
    drop(listener);
    let stop = AtomicBool::new(false);
    let mut ui_frames = 0;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(250));
            stop.store(true, Ordering::Relaxed);
        });
        capture_loop(
            &cli,
            &stop,
            |_| panic!("UI-only opened video"),
            Some(|_, _, bgra: &mut pixels::Bgra8Pixels| {
                ui_frames += 1;
                *bgra.bytes_mut() = [0, 0, 255, 255].repeat(4);
                Ok(())
            }),
        )
        .unwrap();
    });
    assert_eq!(ui_frames, 0);
}
