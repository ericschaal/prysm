use super::*;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::PathBuf;

fn line(text: &str) -> Vec<u8> {
    format!("{text}\n").into_bytes()
}

#[test]
fn commands_have_a_stable_wire_form() {
    for (command, text) in [
        (
            Command::Video {
                video: Video::Available,
            },
            r#"{"type":"video","video":"available"}"#,
        ),
        (Command::ResetVideo {}, r#"{"type":"reset_video"}"#),
        (
            Command::Lighting {
                target: LightingTarget::DeviceNative,
            },
            r#"{"type":"lighting","target":"device_native"}"#,
        ),
    ] {
        assert_eq!(encode(&command).unwrap(), line(text));
        assert_eq!(decode::<Command>(text.as_bytes()).unwrap(), command);
    }
}

#[test]
fn events_round_trip() {
    let status = Event::Status(WorkerStatus {
        capturing: true,
        target: Some(LightingTarget::Processed),
        portal: Some("http://192.168.1.5:8787/".into()),
        failure: None,
        encoding: None,
    });
    let text = encode(&status).unwrap();
    assert!(text.starts_with(br#"{"type":"status","capturing":true"#));
    assert_eq!(decode::<Event>(&text).unwrap(), status);
    let fault = Event::Faulted {
        cause: "release unconfirmed".into(),
    };
    assert_eq!(decode::<Event>(&encode(&fault).unwrap()).unwrap(), fault);
}

#[test]
fn unknown_types_and_fields_are_rejected() {
    for text in [
        r#"{"type":"reboot"}"#,
        r#"{"type":"reset_video","now":true}"#,
        r#"{"type":"video","video":"dim"}"#,
        r#"{"type":"video"}"#,
        r#"{"video":"available"}"#,
        "[]",
        "",
    ] {
        assert!(
            matches!(
                decode::<Command>(text.as_bytes()),
                Err(LinkError::Malformed(_))
            ),
            "{text}"
        );
    }
    for text in [
        r#"{"type":"status","capturing":true,"target":null,"portal":null,"failure":null,"pid":1}"#,
        r#"{"type":"faulted","cause":"x","code":3}"#,
    ] {
        assert!(decode::<Event>(text.as_bytes()).is_err(), "{text}");
    }
}

#[test]
fn line_limit_counts_the_newline() {
    let pad = |len: usize| {
        let mut text = format!(r#"{{"type":"faulted","cause":"{}"}}"#, "");
        let filler = len - text.len() - 1;
        text = format!(r#"{{"type":"faulted","cause":"{}"}}"#, "x".repeat(filler));
        line(&text)
    };
    let fits = pad(MAX_LINE);
    assert_eq!(fits.len(), MAX_LINE);
    let mut reader = LineReader::new(&fits[..]);
    assert!(matches!(
        reader.read::<Event>(),
        Ok(Some(Event::Faulted { .. }))
    ));
    assert!(matches!(reader.read::<Event>(), Ok(None)));

    let over = pad(MAX_LINE + 1);
    let mut reader = LineReader::new(&over[..]);
    assert!(matches!(reader.read::<Event>(), Err(LinkError::Oversized)));
    assert!(matches!(decode::<Event>(&over), Err(LinkError::Oversized)));

    let cause = "x".repeat(MAX_LINE);
    assert!(matches!(
        encode(&Event::Faulted { cause }),
        Err(LinkError::Oversized)
    ));
}

#[test]
fn reader_never_buffers_past_the_limit() {
    // An endless line without a newline must fail after MAX_LINE + 1 bytes.
    let endless = std::io::repeat(b'x');
    let mut reader = LineReader::new(BufReader::new(endless));
    assert!(matches!(reader.read::<Event>(), Err(LinkError::Oversized)));
}

#[test]
fn reader_reports_truncation_and_garbage() {
    let mut reader = LineReader::new(&br#"{"type":"reset_video"}"#[..]);
    assert!(matches!(
        reader.read::<Command>(),
        Err(LinkError::Truncated)
    ));
    let mut reader = LineReader::new(&b"not json\n"[..]);
    assert!(matches!(
        reader.read::<Command>(),
        Err(LinkError::Malformed(_))
    ));
    let both = [
        line(r#"{"type":"reset_video"}"#),
        line(r#"{"type":"reset_video"}"#),
    ]
    .concat();
    let mut reader = LineReader::new(&both[..]);
    assert_eq!(
        reader.read::<Command>().unwrap(),
        Some(Command::ResetVideo {})
    );
    assert_eq!(
        reader.read::<Command>().unwrap(),
        Some(Command::ResetVideo {})
    );
    assert_eq!(reader.read::<Command>().unwrap(), None);
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

#[test]
fn full_args_round_trip_and_reject_mistakes() {
    let args = FullArgs {
        settings: PathBuf::from("/var/lib/io.prysm.tv/settings.json"),
        listen: "0.0.0.0:8787".parse().unwrap(),
        lighting: LightingTarget::DeviceNative,
    };
    let argv = args.to_args();
    assert_eq!(
        argv,
        strings(&[
            "--settings",
            "/var/lib/io.prysm.tv/settings.json",
            "--listen",
            "0.0.0.0:8787",
            "--lighting",
            "device_native"
        ])
    );
    assert_eq!(FullArgs::parse(&argv).unwrap(), args);
    for bad in [
        strings(&["--settings", "s", "--listen", "0.0.0.0:8787"]),
        strings(&["--settings", "s", "--listen", "x", "--lighting", "off"]),
        strings(&[
            "--settings",
            "s",
            "--listen",
            "0.0.0.0:1",
            "--lighting",
            "on",
        ]),
        strings(&[
            "--settings",
            "s",
            "--settings",
            "t",
            "--listen",
            "0.0.0.0:1",
            "--lighting",
            "off",
        ]),
        strings(&["--settings", "s", "--listen", "0.0.0.0:1", "--lighting"]),
        strings(&["full", "--settings", "s"]),
        strings(&["--receiver", "1.2.3.4:5"]),
    ] {
        assert!(FullArgs::parse(&bad).is_err(), "{bad:?}");
    }
}

#[test]
fn slim_args_round_trip_and_validate_capture() {
    let args = SlimArgs {
        receiver: "192.168.1.20:7380".parse::<SocketAddr>().unwrap(),
        capture: SlimCapture::default(),
    };
    let argv = args.to_args();
    assert_eq!(
        argv,
        strings(&[
            "--receiver",
            "192.168.1.20:7380",
            "--width",
            "320",
            "--height",
            "180",
            "--fps",
            "30",
            "--include-ui",
            "true",
            "--encoding",
            "h264_hardware"
        ])
    );
    assert_eq!(SlimArgs::parse(&argv).unwrap(), args);
    let mut odd = argv.clone();
    odd[3] = "321".into();
    assert!(SlimArgs::parse(&odd).unwrap_err().contains("capture.width"));
    let mut fast = argv.clone();
    fast[7] = "61".into();
    assert!(SlimArgs::parse(&fast).unwrap_err().contains("capture.fps"));
    let mut maybe = argv.clone();
    maybe[9] = "maybe".into();
    assert!(SlimArgs::parse(&maybe).is_err());
    for encoding in [SlimEncoding::H264Software, SlimEncoding::Raw] {
        let mut other = argv.clone();
        other[11] = encoding.to_string();
        assert_eq!(SlimArgs::parse(&other).unwrap().capture.encoding, encoding);
    }
    let mut unknown = argv.clone();
    unknown[11] = "h265".into();
    assert!(
        SlimArgs::parse(&unknown)
            .unwrap_err()
            .contains("--encoding")
    );
    assert!(
        SlimArgs::parse(&argv[..10])
            .unwrap_err()
            .contains("--encoding")
    );
    // Stored preferences without the field default to hardware H.264.
    let stored: SlimCapture = serde_json::from_str(r#"{"fps": 15}"#).unwrap();
    assert_eq!(stored.encoding, SlimEncoding::H264Hardware);
    let soft: SlimCapture = serde_json::from_str(r#"{"encoding": "h264_software"}"#).unwrap();
    assert_eq!(soft.encoding, SlimEncoding::H264Software);
    assert!(serde_json::from_str::<SlimCapture>(r#"{"encoding": "lz4"}"#).is_err());
}

#[test]
fn exit_codes_are_described() {
    assert_eq!(exit::describe(3), "worker faulted");
    assert!(exit::describe(2).contains("packaging"));
    assert_eq!(exit::describe(-1), "worker exited with code -1");
}
