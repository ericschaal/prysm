use super::*;
use prysm_ports::StreamEncoding;
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
        (
            Command::Video {
                video: Video::Refused,
            },
            r#"{"type":"video","video":"refused"}"#,
        ),
        (Command::ResetVideo {}, r#"{"type":"reset_video"}"#),
        (
            Command::Target {
                target: LightingTarget::Effects,
                sequence: 3,
            },
            r#"{"type":"target","target":"effects","sequence":3}"#,
        ),
    ] {
        assert_eq!(encode(&command).unwrap(), line(text));
        assert_eq!(decode::<Command>(text.as_bytes()).unwrap(), command);
    }
}

#[test]
fn events_round_trip() {
    let full = Event::Status(WorkerStatus::Full(FullStatus {
        capturing: true,
        fps: Some(30),
        target: LightingTarget::ScreenSync,
        applied: 2,
        portal: Some("http://192.168.1.5:8787/".into()),
        failure: Some(WorkerFailure::other("output: refused")),
    }));
    let text = encode(&full).unwrap();
    assert_eq!(
        String::from_utf8(text.clone()).unwrap(),
        "{\"type\":\"status\",\"flavor\":\"full\",\"capturing\":true,\"fps\":30,\
         \"target\":\"screen_sync\",\"applied\":2,\"portal\":\"http://192.168.1.5:8787/\",\
         \"failure\":{\"kind\":\"other\",\"message\":\"output: refused\"}}\n"
    );
    assert_eq!(decode::<Event>(&text).unwrap(), full);
    assert_eq!(
        String::from_utf8(
            encode(&Event::Status(WorkerStatus::Slim(SlimStatus::default()))).unwrap()
        )
        .unwrap(),
        "{\"type\":\"status\",\"flavor\":\"slim\",\"capturing\":false,\"link\":{\"state\":\"starting\"}}\n",
    );
    for (link, text) in [
        (
            SlimLink::Sending {
                encoding: StreamEncoding::Raw,
                fps: 30,
            },
            r#"{"state":"sending","encoding":"raw","fps":30}"#,
        ),
        (SlimLink::Paused {}, r#"{"state":"paused"}"#),
        (SlimLink::Refused {}, r#"{"state":"refused"}"#),
        (
            SlimLink::Waiting {
                failure: WorkerFailure::new(FailureKind::Receiver, "refused"),
            },
            r#"{"state":"waiting","failure":{"kind":"receiver","message":"refused"}}"#,
        ),
        (
            SlimLink::Failed {
                failure: WorkerFailure::new(FailureKind::Encoder, "busy"),
            },
            r#"{"state":"failed","failure":{"kind":"encoder","message":"busy"}}"#,
        ),
    ] {
        let status = Event::Status(WorkerStatus::Slim(SlimStatus {
            capturing: true,
            link,
        }));
        let encoded = encode(&status).unwrap();
        assert_eq!(
            String::from_utf8(encoded.clone()).unwrap(),
            format!(r#"{{"type":"status","flavor":"slim","capturing":true,"link":{text}}}"#) + "\n"
        );
        assert_eq!(decode::<Event>(&encoded).unwrap(), status);
    }
    let failed = Event::Failed {
        cause: "cannot bind 0.0.0.0:8787".into(),
    };
    let text = encode(&failed).unwrap();
    assert_eq!(
        text,
        line(r#"{"type":"failed","cause":"cannot bind 0.0.0.0:8787"}"#)
    );
    assert_eq!(decode::<Event>(&text).unwrap(), failed);
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
    const SLIM: &str = r#""capturing":false,"link":{"state":"starting"}"#;
    let slim_link = |link: &str| {
        format!(r#"{{"type":"status","flavor":"slim","capturing":false,"link":{link}}}"#)
    };
    const FULL: &str = r#""capturing":false,"target":"off","applied":0"#;
    // Both base lines are valid, so each rejection below is for its change.
    for valid in [
        format!(r#"{{"type":"status","flavor":"slim",{SLIM}}}"#),
        format!(r#"{{"type":"status","flavor":"full",{FULL}}}"#),
    ] {
        assert!(decode::<Event>(valid.as_bytes()).is_ok(), "{valid}");
    }
    for text in [
        format!(r#"{{"type":"status","flavor":"slim",{SLIM},"pid":1}}"#),
        format!(r#"{{"type":"status","flavor":"full",{FULL},"pid":1}}"#),
        r#"{"type":"faulted","cause":"x","code":3}"#.to_owned(),
        // No flavour, an unknown one, or the other flavour's fields.
        format!(r#"{{"type":"status",{SLIM}}}"#),
        format!(r#"{{"type":"status","flavor":"tv",{SLIM}}}"#),
        format!(r#"{{"type":"status","flavor":"full",{SLIM}}}"#),
        format!(r#"{{"type":"status","flavor":"slim",{FULL}}}"#),
        format!(r#"{{"type":"status","flavor":"full",{FULL},"link":{{"state":"paused"}}}}"#),
        format!(r#"{{"type":"status","flavor":"slim",{SLIM},"portal":"x"}}"#),
        // No link, an unknown state, or a field another state has.
        r#"{"type":"status","flavor":"slim","capturing":false}"#.to_owned(),
        r#"{"type":"status","flavor":"slim","capturing":false,"link":"sending"}"#.to_owned(),
        format!(r#"{{"type":"status","flavor":"slim",{SLIM},"paused":false}}"#),
        slim_link(r#"{"state":"lost"}"#),
        slim_link(r#"{"state":"paused","fps":1}"#),
        slim_link(r#"{"state":"starting","failure":{"kind":"other","message":"x"}}"#),
        slim_link(r#"{"state":"sending","encoding":"raw"}"#),
        // The failure as text, without its kind, or with another field.
        slim_link(r#"{"state":"waiting","failure":"x"}"#),
        slim_link(r#"{"state":"failed","failure":{"message":"x"}}"#),
        slim_link(r#"{"state":"failed","failure":{"kind":"other","message":"x","at":1}}"#),
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

/// What the worker parses from `argv` (after its program name).
fn parse<A: clap::Parser>(argv: &[String]) -> Result<A, clap::Error> {
    A::try_parse_from(std::iter::once("worker").chain(argv.iter().map(String::as_str)))
}

#[test]
fn full_args_round_trip_and_reject_mistakes() {
    let args = FullArgs {
        settings: PathBuf::from("/var/lib/prysm/settings.json"),
        listen: "0.0.0.0:8787".parse().unwrap(),
        target: LightingTarget::Effects,
    };
    let argv = args.to_args();
    assert_eq!(
        argv,
        strings(&[
            "--settings",
            "/var/lib/prysm/settings.json",
            "--listen",
            "0.0.0.0:8787",
            "--target",
            "effects"
        ])
    );
    assert_eq!(parse::<FullArgs>(&argv).unwrap(), args);
    for target in LightingTarget::ALL {
        let args = FullArgs {
            listen: "[::1]:0".parse().unwrap(),
            target,
            ..args.clone()
        };
        assert_eq!(parse::<FullArgs>(&args.to_args()).unwrap(), args);
    }
    for bad in [
        strings(&["--settings", "s", "--listen", "0.0.0.0:8787"]),
        strings(&["--settings", "s", "--listen", "x", "--target", "off"]),
        strings(&["--settings", "s", "--listen", "0.0.0.0:1", "--target", "on"]),
        strings(&[
            "--settings",
            "s",
            "--settings",
            "t",
            "--listen",
            "0.0.0.0:1",
            "--target",
            "off",
        ]),
        strings(&["--settings", "s", "--listen", "0.0.0.0:1", "--target"]),
        strings(&["full", "--settings", "s"]),
        strings(&["--prysm", "http://1.2.3.4:5"]),
    ] {
        assert!(parse::<FullArgs>(&bad).is_err(), "{bad:?}");
    }
}

#[test]
fn slim_args_carry_only_the_prysm_address() {
    let args = SlimArgs {
        prysm: "http://192.168.1.20:8787".parse().unwrap(),
    };
    let argv = args.to_args();
    assert_eq!(argv, strings(&["--prysm", "http://192.168.1.20:8787"]));
    assert_eq!(parse::<SlimArgs>(&argv).unwrap(), args);
    assert!(parse::<SlimArgs>(&strings(&["--prysm", "prysm.local:8787"])).is_err());
    assert!(
        parse::<SlimArgs>(&strings(&[
            "--prysm",
            "http://192.168.1.20:8787",
            "--bogus"
        ]))
        .is_err()
    );
    let error = parse::<SlimArgs>(&[]).unwrap_err().to_string();
    assert!(error.contains("--prysm"), "{error}");
    // A host name is resolved by the worker, not here.
    let named = parse::<SlimArgs>(&strings(&["--prysm", "https://prysm.example.com"])).unwrap();
    assert_eq!(named.prysm.host(), "prysm.example.com");
    assert_eq!(
        named.to_args(),
        strings(&["--prysm", "https://prysm.example.com"])
    );
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
