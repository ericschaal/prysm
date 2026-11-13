use super::*;
use std::io::BufReader;
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
        fps: Some(30),
        target: Some(LightingTarget::Processed),
        portal: Some("http://192.168.1.5:8787/".into()),
        failure: Some(WorkerFailure::new(
            FailureKind::Receiver,
            "waiting for the receiver: refused",
        )),
        encoding: None,
        paused: false,
    });
    let text = encode(&status).unwrap();
    assert!(
        String::from_utf8_lossy(&text).contains(
            r#""failure":{"kind":"receiver","message":"waiting for the receiver: refused"}"#
        ),
        "{}",
        String::from_utf8_lossy(&text)
    );
    assert!(text.starts_with(br#"{"type":"status","capturing":true,"fps":30"#));
    assert_eq!(decode::<Event>(&text).unwrap(), status);
    assert_eq!(
        String::from_utf8(encode(&Event::Status(WorkerStatus::default())).unwrap()).unwrap(),
        "{\"type\":\"status\",\"capturing\":false,\"fps\":null,\"target\":null,\
         \"portal\":null,\"failure\":null,\"encoding\":null,\"paused\":false}\n"
    );
    let paused = Event::Status(WorkerStatus {
        paused: true,
        ..WorkerStatus::default()
    });
    let text = encode(&paused).unwrap();
    assert!(String::from_utf8_lossy(&text).contains(r#""paused":true"#));
    assert_eq!(decode::<Event>(&text).unwrap(), paused);
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
        r#"{"type":"status","capturing":true,"paused":false,"pid":1}"#,
        r#"{"type":"faulted","cause":"x","code":3}"#,
        // The failure as two fields, or a failure without its kind.
        r#"{"type":"status","capturing":false,"paused":false,"failure":"x","failure_kind":"other"}"#,
        r#"{"type":"status","capturing":false,"paused":false,"failure":{"message":"x"}}"#,
        r#"{"type":"status","capturing":false,"paused":false,"failure":{"kind":"other","message":"x","at":1}}"#,
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
fn slim_args_carry_only_the_receiver() {
    let args = SlimArgs {
        receiver: "192.168.1.20:7380".parse().unwrap(),
    };
    let argv = args.to_args();
    assert_eq!(argv, strings(&["--receiver", "192.168.1.20:7380"]));
    assert_eq!(SlimArgs::parse(&argv).unwrap(), args);
    // Capture and encoding are the receiver's request, not arguments.
    let mut old = argv.clone();
    old.extend(strings(&["--width", "320"]));
    assert!(SlimArgs::parse(&old).unwrap_err().contains("--width"));
    assert!(SlimArgs::parse(&strings(&["--receiver", "nope"])).is_err());
    assert!(SlimArgs::parse(&[]).unwrap_err().contains("--receiver"));
    // A host name is resolved by the worker, not here.
    let named = SlimArgs::parse(&strings(&["--receiver", "prysm.local:7380"])).unwrap();
    assert_eq!(named.receiver.host(), "prysm.local");
    assert_eq!(
        named.to_args(),
        strings(&["--receiver", "prysm.local:7380"])
    );
}

#[test]
fn receiver_addresses_are_ip_addresses_or_host_names_with_a_fixed_port() {
    for (text, host, ip) in [
        ("192.168.1.20:7380", "192.168.1.20", true),
        ("[fe80::1]:7380", "fe80::1", true),
        ("prysm.local:7380", "prysm.local", false),
        ("Prysm-Desktop:7380", "Prysm-Desktop", false),
        ("lights.example.com:1", "lights.example.com", false),
        ("localhost:65535", "localhost", false),
    ] {
        let address: ReceiverAddress = text.parse().unwrap();
        assert_eq!(address.host(), host, "{text}");
        assert_eq!(address.to_string(), text, "round trip");
        assert_eq!(address.socket_addr().is_some(), ip, "{text}");
        assert_eq!(
            serde_json::to_value(&address).unwrap(),
            serde_json::Value::String(text.into())
        );
    }
    assert_eq!(
        "192.168.1.20:7380"
            .parse::<ReceiverAddress>()
            .unwrap()
            .socket_addr(),
        Some("192.168.1.20:7380".parse().unwrap())
    );
    for (bad, reason) in [
        ("192.168.1.20", "host:port"),
        ("prysm.local", "host:port"),
        ("prysm.local:0", "fixed port"),
        ("prysm.local:70000", "fixed port"),
        ("prysm.local:", "fixed port"),
        (":7380", "not a host name"),
        ("0.0.0.0:7380", "not 0.0.0.0"),
        ("[::]:7380", "not ::"),
        ("fe80::1:7380", "not a host name"),
        ("[nope]:7380", "not an IPv6 address"),
        ("192.168.1:7380", "not a host name"),
        ("300.1.1.1:7380", "not a host name"),
        ("prysm.local.:7380", "not a host name"),
        ("-prysm:7380", "not a host name"),
        ("prysm_tv:7380", "not a host name"),
        ("http://prysm.local:7380", "not a host name"),
        ("prysm local:7380", "not a host name"),
    ] {
        let error = bad.parse::<ReceiverAddress>().unwrap_err();
        assert!(error.contains(reason), "{bad}: {error}");
    }
    let long = format!("{}.local:7380", "a".repeat(250));
    assert!(long.parse::<ReceiverAddress>().is_err());
    // serde names the field's value, for the Luna error text.
    let error = serde_json::from_value::<ReceiverAddress>(serde_json::json!("0.0.0.0:7380"))
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("receiver `0.0.0.0:7380`: "), "{error}");
}

#[test]
fn slim_encodings_have_their_serde_names() {
    for encoding in [
        SlimEncoding::H264Hardware,
        SlimEncoding::H264Software,
        SlimEncoding::Raw,
    ] {
        assert_eq!(encoding.to_string().parse(), Ok(encoding));
        assert_eq!(
            serde_json::to_value(encoding).unwrap(),
            serde_json::Value::String(encoding.to_string())
        );
    }
    assert!("h265".parse::<SlimEncoding>().is_err());
}

#[test]
fn backoff_doubles_to_the_cap_and_resets() {
    let mut backoff = Backoff::default();
    let delays: Vec<u64> = (0..7).map(|_| backoff.delay().as_secs()).collect();
    assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30]);
    backoff.reset();
    assert_eq!(backoff.delay(), Backoff::MIN);
    let mut short = Backoff::starting_at(std::time::Duration::from_millis(20));
    assert_eq!(short.delay().as_millis(), 20);
    assert_eq!(short.delay().as_millis(), 40);
    short.reset();
    assert_eq!(short.delay().as_millis(), 20);
}

#[test]
fn exit_codes_are_described() {
    assert_eq!(exit::describe(3), "worker faulted");
    assert!(exit::describe(2).contains("packaging"));
    assert_eq!(exit::describe(-1), "worker exited with code -1");
}
