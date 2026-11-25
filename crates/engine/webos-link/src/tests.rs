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
        lights: Some(OutputIssue::Unreachable),
        applied: 2,
        portal: Some("http://192.168.1.5:8787/".into()),
        failure: Some(Failure {
            escalation: Escalation::Unanswered,
            message: "output: refused".into(),
        }),
    }));
    let text = encode(&full).unwrap();
    assert_eq!(
        String::from_utf8(text.clone()).unwrap(),
        "{\"type\":\"status\",\"flavor\":\"full\",\"capturing\":true,\"fps\":30,\
         \"target\":\"screen_sync\",\"lights\":\"unreachable\",\"applied\":2,\"portal\":\"http://192.168.1.5:8787/\",\
         \"failure\":{\"escalation\":{\"step\":\"unanswered\"},\"message\":\"output: refused\"}}\n"
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
                message: "refused".into(),
            },
            r#"{"state":"waiting","message":"refused"}"#,
        ),
        (
            SlimLink::EncoderFailed {
                message: "busy".into(),
            },
            r#"{"state":"encoder_failed","message":"busy"}"#,
        ),
        (
            SlimLink::VersionDiffers {
                message: "v2".into(),
            },
            r#"{"state":"version_differs","message":"v2"}"#,
        ),
        (
            SlimLink::Replaced {
                message: "r".into(),
            },
            r#"{"state":"replaced","message":"r"}"#,
        ),
        (
            SlimLink::ReceiverFailed {
                message: "f".into(),
            },
            r#"{"state":"receiver_failed","message":"f"}"#,
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
    for (escalation, text) in [
        (
            Escalation::FAIL_FAST,
            r#"{"type":"failed","escalation":{"step":"ask","fix":"try_again"},"message":"m"}"#,
        ),
        (
            Escalation::Ask(Fix::ChangeSettings),
            r#"{"type":"failed","escalation":{"step":"ask","fix":"change_settings"},"message":"m"}"#,
        ),
        (
            Escalation::Restart,
            r#"{"type":"failed","escalation":{"step":"restart"},"message":"m"}"#,
        ),
        (
            Escalation::Unanswered,
            r#"{"type":"failed","escalation":{"step":"unanswered"},"message":"m"}"#,
        ),
    ] {
        let failed = Event::Failed(Failure {
            escalation,
            message: "m".into(),
        });
        let encoded = encode(&failed).unwrap();
        assert_eq!(encoded, line(text));
        assert_eq!(decode::<Event>(&encoded).unwrap(), failed);
    }
}

/// A failure is its escalation and message: what `prysm_ports::Error` says.
#[test]
fn a_failure_copies_the_errors_escalation_and_chain() {
    let error = prysm_ports::Error::Faulted(prysm_ports::SessionPhase::SourceUnreleased);
    assert_eq!(
        Failure::of(&error),
        Failure {
            escalation: Escalation::Restart,
            message: prysm_ports::chain(&error),
        }
    );
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
        // The failure without its escalation, as text, or with another field.
        r#"{"type":"failed","cause":"x"}"#.to_owned(),
        r#"{"type":"failed","message":"x"}"#.to_owned(),
        r#"{"type":"failed","escalation":"restart","message":"x"}"#.to_owned(),
        r#"{"type":"failed","escalation":{"step":"restart"},"message":"x","code":3}"#.to_owned(),
        r#"{"type":"failed","escalation":{"step":"ask"},"message":"x"}"#.to_owned(),
        r#"{"type":"faulted","cause":"x"}"#.to_owned(),
        format!(
            r#"{{"type":"status","flavor":"full",{FULL},"failure":{{"kind":"other","message":"x"}}}}"#
        ),
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
    let failed = |message: &str| {
        format!(r#"{{"type":"failed","escalation":{{"step":"restart"}},"message":"{message}"}}"#)
    };
    let pad = |len: usize| {
        let filler = len - failed("").len() - 1;
        line(&failed(&"x".repeat(filler)))
    };
    let fits = pad(MAX_LINE);
    assert_eq!(fits.len(), MAX_LINE);
    let mut reader = LineReader::new(&fits[..]);
    assert!(matches!(reader.read::<Event>(), Ok(Some(Event::Failed(_)))));
    assert!(matches!(reader.read::<Event>(), Ok(None)));

    let over = pad(MAX_LINE + 1);
    let mut reader = LineReader::new(&over[..]);
    assert!(matches!(reader.read::<Event>(), Err(LinkError::Oversized)));
    assert!(matches!(decode::<Event>(&over), Err(LinkError::Oversized)));

    let message = "x".repeat(MAX_LINE);
    assert!(matches!(
        encode(&Event::Failed(Failure {
            escalation: Escalation::Restart,
            message
        })),
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
