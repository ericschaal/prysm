//! Actual serializations of every wire root, validated against the generated
//! schemas by the frontend contract tests.
use prysm_capture_device::{DeviceSettings, DeviceSource};
use prysm_capture_file::{FileSettings, FileSource};
use prysm_capture_remote::{RemoteReceiver, RemoteSettings, StreamEncoding};
use prysm_capture_webos::{WebosCapture, WebosSettings};
use prysm_output_wled::{LedCounts, Wled, WledSettings};
use prysm_ports::{
    AdapterSettings, Behind, Calibration, CalibrationPattern, Candidate, CorrectionControls,
    DeviceBrightness, DeviceReport, Error, Input, Latency, LightingTarget, Mode, Modes,
    OutputIssue, Pattern, PrimaryControls, Processing, Revision, Section, SessionPhase,
    SessionState, Settings, Snapshot, Span, Style, StyleValues, ValidationError,
};
use prysm_ports::{OutputFactory, SourceFactory};
use prysm_protocol::{
    ColorsEvent, Failure, LedColor, LedColors, PreviewEvent, PreviewFrame, PreviewViewport,
    SettingsReplaced, SettingsRequest, State, TargetRequest,
};
use prysm_runtime::{NullOutput, NullSettings};
use prysm_webos_link::{FailureKind, SlimEncoding, Video, WorkerFailure};
use prysm_webos_service::preferences::Preferences;
use prysm_webos_service::service::{
    GetStateRequest, SetEnabledRequest, UninstallRequest, state_reply,
};
use prysm_webos_service::state::{FullWorker, Package, Power, SlimWorker, State as TvState};
use serde_json::{Value, json};

/// A fixture key `module/Name` names a schema of one document; a plain
/// `Name` is looked up in every document.
pub fn serialized() -> Value {
    let mut fixtures = serde_json::Map::new();
    for document in [application(), adapter_sections(), luna()] {
        // Each is a `json!({ .. })` object.
        if let Value::Object(document) = document {
            fixtures.extend(document);
        }
    }
    Value::Object(fixtures)
}

/// The TV supervisor's Luna values: a state for each package in each worker
/// phase.
fn luna() -> Value {
    let slim_preferences = Preferences {
        autostart: true,
        instance: Some("http://192.168.1.20:8787".parse().expect("a Prysm address")),
    };
    let portal = || Some("http://192.168.1.5:8787/".to_owned());
    let full = |lighting, worker| Package::Full { lighting, worker };
    let slim = |enabled, worker| Package::Slim { enabled, worker };
    let running = full(
        LightingTarget::Processed,
        FullWorker::Running {
            capturing: true,
            fps: Some(30),
            portal: portal(),
        },
    );
    let full_states = [
        running.clone(),
        full(LightingTarget::Off, FullWorker::Stopped),
        full(
            LightingTarget::DeviceNative,
            FullWorker::Stopping { portal: portal() },
        ),
        full(
            LightingTarget::Processed,
            FullWorker::Retrying { seconds: 2 },
        ),
    ]
    .map(|package| TvState {
        package,
        preferences: Preferences::default(),
        power: Power::Active,
        video: Video::Available,
        failure: None,
        log: vec!["prysm-webos: portal on 0.0.0.0:8787".into()],
    });
    let encoder = WorkerFailure::new(
        FailureKind::Encoder,
        "cannot encode frame: hardware H.264 encoder cannot start: rvenc unavailable",
    );
    let slim_states = [
        (
            slim(true, SlimWorker::Retrying { seconds: 4 }),
            Some(encoder),
        ),
        (
            slim(
                true,
                SlimWorker::Running {
                    capturing: false,
                    fps: None,
                    encoding: Some(SlimEncoding::H264Hardware),
                    paused: true,
                },
            ),
            None,
        ),
        (slim(false, SlimWorker::Stopped), None),
        (slim(false, SlimWorker::Stopping), None),
    ]
    .map(|(package, failure)| TvState {
        package,
        preferences: slim_preferences.clone(),
        power: Power::ScreenSaver,
        video: Video::Unknown,
        failure,
        log: Vec::new(),
    });
    let (full, slim) = (&full_states[0], &slim_states[0]);
    let states: Vec<&TvState> = full_states.iter().chain(&slim_states).collect();
    json!({
        "luna/State": states,
        "luna/Preferences": [Preferences::default(), slim_preferences],
        "luna/StateReply": [state_reply(full, Some(true)), state_reply(slim, None)],
        "luna/GetStateRequest": [GetStateRequest { subscribe: true }],
        "luna/SetEnabledRequest": [SetEnabledRequest { enabled: false }],
        "luna/UninstallRequest": [UninstallRequest {}],
    })
}

/// Two saved calibrations, the first active.
fn with_calibrations() -> Settings {
    Settings {
        processing: Processing {
            calibrations: vec![
                Calibration {
                    id: "ps5".into(),
                    name: "PS5".into(),
                    correction: CorrectionControls {
                        warmth: 12,
                        tint: -4,
                        midtones: 6,
                        red: PrimaryControls {
                            hue: -5,
                            strength: -15,
                        },
                        ..CorrectionControls::default()
                    },
                },
                Calibration {
                    id: "0b6f2c1e-1d2a-4c3b-9f4e-5a6b7c8d9e0f".into(),
                    name: "TV apps".into(),
                    correction: CorrectionControls::default(),
                },
            ],
            active_calibration: Some("ps5".into()),
            ..Processing::default()
        },
        ..Settings::default()
    }
}

/// The user's own style values, applied with `custom` and kept with
/// another style.
fn with_custom_style(style: Style) -> Settings {
    Settings {
        processing: Processing {
            style,
            custom_style: Some(StyleValues {
                smoothing_seconds: 0.5,
                cut_response_percent: 70,
                brightness_ramp_seconds: 0.5,
                spatial_reach_percent: 15,
                color_focus_percent: 60,
                saturation_boost_percent: 40,
                dark_floor_percent: 10,
            }),
            ..Processing::default()
        },
        ..Settings::default()
    }
}

/// A configured device capture into WLED, as raw adapter sections.
fn configured() -> Settings {
    Settings {
        capture: Some(AdapterSettings::new(
            "device",
            &json!({ "path": "/dev/video0" }),
        )),
        output: Some(AdapterSettings::new(
            "wled",
            &json!({ "address": "192.0.2.1", "leds": { "top": 10, "left": 4 } }),
        )),
        processing: Processing::default(),
    }
}

/// Idle, running (with an output issue, a surface failure, or calibrating),
/// device-native and faulted.
fn states() -> Vec<State> {
    let snapshots = [
        Snapshot::default(),
        Snapshot {
            target: LightingTarget::Processed,
            session: SessionState::Running,
            settings: configured(),
            revision: Revision(7),
            input: Some(Input {
                fps: 29.9,
                receiving: true,
                sent_fps: 29.9,
                latency: Some(Latency {
                    capture_ms: Some(88.0),
                    output_ms: 2.4,
                }),
                behind: Behind::Output,
            }),
            device: DeviceReport {
                issue: Some(OutputIssue::Overridden),
                brightness: DeviceBrightness::from_percent(40),
                led_count: Some(120),
            },
            surface_failures: vec![Error::other("portal listener failed")],
            ..Snapshot::default()
        },
        Snapshot {
            target: LightingTarget::Processed,
            session: SessionState::Running,
            settings: with_calibrations(),
            calibration: Some(CalibrationPattern {
                pattern: Pattern::Gray192,
                uncorrected: false,
            }),
            ..Snapshot::default()
        },
        Snapshot {
            target: LightingTarget::DeviceNative,
            session: SessionState::Native,
            ..Snapshot::default()
        },
        // A start that failed from the lights' own effects: the session is
        // still native, the failure says the capture's side.
        Snapshot {
            target: LightingTarget::Processed,
            session: SessionState::Native,
            settings: configured(),
            failure: Some(Error::other("video device capture failed").in_section(Section::Capture)),
            ..Snapshot::default()
        },
        Snapshot {
            target: LightingTarget::Off,
            session: SessionState::Faulted(SessionPhase::IdlingOutput),
            failure: Some(Error::Faulted(SessionPhase::IdlingOutput)),
            ..Snapshot::default()
        },
    ];
    snapshots
        .iter()
        .map(|snapshot| State {
            // Fixed, so a release (a new `VERSION`) leaves the fixtures as they are.
            version: "1.0.0".into(),
            ..State::from(snapshot)
        })
        .collect()
}

fn failures() -> Vec<Failure> {
    let errors = [
        Error::from(ValidationError::new(
            "output.address",
            "a unicast device address is required",
        )),
        Error::StaleRevision {
            expected: Revision(1),
            current: Revision(2),
        },
        Error::NotProcessed,
        Error::other("device unplugged"),
        Error::other("capture device does not support 640x400 at 30 Hz")
            .in_section(Section::Capture),
        Error::other("WLED did not answer").in_section(Section::Output),
        Error::Closed,
    ];
    errors.iter().map(Failure::from).collect()
}

fn preview_events() -> Vec<PreviewEvent> {
    vec![
        PreviewEvent::Idle,
        PreviewEvent::Unavailable {
            message: "MJPEG frames cannot be previewed".into(),
        },
        PreviewEvent::Frame(PreviewFrame {
            sequence: 42,
            source_width: 1920,
            source_height: 1080,
            width: 480,
            height: 270,
            viewport: PreviewViewport {
                x: 0,
                y: 140,
                width: 1920,
                height: 800,
            },
            colors: LedColors {
                top: vec![LedColor([255, 0, 0]), LedColor([0, 255, 0])],
                right: vec![LedColor([0, 0, 255])],
                bottom: vec![LedColor([188, 188, 188])],
                left: vec![LedColor([0, 0, 0])],
            },
            jpeg: "/9j/2wBDAP/Z".into(),
        }),
    ]
}

fn colors_events() -> Vec<ColorsEvent> {
    vec![
        ColorsEvent::Idle,
        ColorsEvent::Colors(LedColors {
            top: vec![LedColor([255, 0, 0]), LedColor([0, 255, 0])],
            right: vec![LedColor([0, 0, 255])],
            bottom: vec![LedColor([188, 188, 188])],
            left: vec![LedColor([0, 0, 0])],
        }),
    ]
}

/// The portal's wire values.
fn application() -> Value {
    json!({
        "State": states(),
        "PreviewEvent": preview_events(),
        "ColorsEvent": colors_events(),
        "Failure": failures(),
        "TargetRequest": [TargetRequest { target: LightingTarget::Processed }],
        // Every pattern, as `PUT /api/v1/calibration` bodies.
        "CalibrationPattern": Pattern::ALL
            .into_iter()
            .enumerate()
            .map(|(index, pattern)| CalibrationPattern { pattern, uncorrected: index % 2 == 1 })
            .collect::<Vec<_>>(),
        "SettingsRequest": [
            SettingsRequest { expected: Revision(0), settings: Settings::default() },
            SettingsRequest { expected: Revision(7), settings: configured() },
            SettingsRequest { expected: Revision(8), settings: with_calibrations() },
        ],
        "SettingsReplaced": [SettingsReplaced { revision: Revision(8) }],
        "Settings": [
            Settings::default(),
            with_calibrations(),
            with_custom_style(Style::Custom),
            with_custom_style(Style::Vivid),
        ],
        "Style": Style::BUILT_IN.into_iter().chain([Style::Custom]).collect::<Vec<_>>(),
        "StyleValues": Style::BUILT_IN.into_iter().filter_map(Style::built_in).collect::<Vec<_>>(),
        // `POST /api/v1/capture/modes` replies: a scaling capture's range,
        // a camera's fixed modes, none, unknown.
        "Modes": [
            Modes::from(prysm_capture_webos::MODE),
            Modes::from(prysm_ports::capture_size::SIZES),
            Modes::Known(vec![
                Mode::new(Span::exactly(640), Span::exactly(360), Span::new(1, 30, 1).expect("a span")),
                Mode::new(Span::exactly(1920), Span::exactly(1080), Span::exactly(30)),
            ]),
            Modes::Known(Vec::new()),
            Modes::Unknown,
        ],
        "Candidate": [
            Candidate::new("USB Video", &json!({ "path": "/dev/video0" })).detail("/dev/video0"),
            Candidate::new("Living room", &json!({ "address": "192.168.1.40" }))
                .detail("192.168.1.40")
                .led_count(120),
        ],
    })
}

/// The host documents' adapter sections, as the registry stores them.
fn adapter_sections() -> Value {
    json!({
        "DeviceSettings": [AdapterSettings::new(
            DeviceSource::KIND,
            &DeviceSettings::new("/dev/video0"),
        )],
        "FileSettings": [
            AdapterSettings::new(FileSource::KIND, &FileSettings::new("/videos/clip.mkv")),
            AdapterSettings::new(
                FileSource::KIND,
                &FileSettings { width: 320, height: 180, fps: 24, ..FileSettings::new("clip.mp4") },
            ),
        ],
        "RemoteReceiverSettings": [
            AdapterSettings::new(RemoteReceiver::KIND, &RemoteSettings::default()),
            AdapterSettings::new(
                RemoteReceiver::KIND,
                &RemoteSettings {
                    width: 640,
                    height: 280,
                    fps: 25,
                    include_ui: false,
                    encoding: StreamEncoding::H264Software,
                },
            ),
        ],
        "WledSettings": [AdapterSettings::new(
            Wled::KIND,
            &WledSettings {
                address: "192.0.2.1".into(),
                leds: LedCounts { top: 10, left: 4, ..LedCounts::default() },
                dither: true,
            },
        )],
        "WebosSettings": [AdapterSettings::new(
            WebosCapture::KIND,
            &WebosSettings { fps: 60, include_ui: false, ..WebosSettings::default() },
        )],
        "NullSettings": [AdapterSettings::new(NullOutput::KIND, &NullSettings::new())],
    })
}
