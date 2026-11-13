//! Actual serializations of every wire root, validated against the generated
//! schemas by the frontend contract tests.
use prysm_capture_device::{DeviceSettings, DeviceSource};
use prysm_capture_file::{FileSettings, FileSource};
use prysm_capture_remote::{RemoteReceiver, RemoteReceiverSettings, StreamEncoding, StreamRequest};
use prysm_capture_webos::{WebosCapture, WebosSettings};
use prysm_output_wled::{LedCounts, Wled, WledSettings};
use prysm_ports::{
    AdapterSettings, Calibration, Candidate, ColorProfile, CorrectionControls, Error, HueControls,
    LightingTarget, OutputIssue, Pattern, Processing, Revision, Section, SessionPhase,
    SessionState, Settings, Snapshot, ValidationError,
};
use prysm_ports::{OutputFactory, SourceFactory};
use prysm_protocol::{
    ColorsEvent, Failure, LedColor, LedColors, PreviewEvent, PreviewFrame, PreviewViewport,
    SettingsReplaced, SettingsRequest, State, TargetRequest,
};
use prysm_runtime::{NullOutput, NullSettings};
use prysm_webos_link::{FailureKind, Video, WorkerFailure};
use prysm_webos_service::preferences::Preferences;
use prysm_webos_service::service::{
    GetStateRequest, SetEnabledRequest, UninstallRequest, state_reply,
};
use prysm_webos_service::state::{Flavor, Phase, Power, State as TvState};
use serde_json::{Value, json};
use std::net::SocketAddr;

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

/// The TV supervisor's Luna values, one representative state per flavor.
fn luna() -> Value {
    let slim_preferences = Preferences {
        autostart: true,
        instance: Some("http://192.168.1.20:8787".into()),
        receiver: Some("prysm.local:7380".parse().expect("a receiver address")),
    };
    let full = TvState {
        flavor: Flavor::Full,
        preferences: Preferences::default(),
        enabled: true,
        lighting: Some(LightingTarget::Processed),
        power: Power::Active,
        video: Video::Available,
        worker: Phase::Running,
        capturing: true,
        fps: Some(30),
        portal: Some("http://192.168.1.5:8787/".into()),
        failure: None,
        encoding: None,
        paused: false,
        log: vec!["prysm-webos: portal on 0.0.0.0:8787".into()],
    };
    let slim = TvState {
        flavor: Flavor::Slim,
        preferences: slim_preferences.clone(),
        enabled: true,
        lighting: None,
        power: Power::ScreenSaver,
        video: Video::Unknown,
        worker: Phase::Retrying { seconds: 4 },
        capturing: false,
        fps: None,
        portal: None,
        failure: Some(WorkerFailure::new(
            FailureKind::Encoder,
            "cannot encode frame: hardware H.264 encoder cannot start: rvenc unavailable",
        )),
        encoding: None,
        paused: false,
        log: Vec::new(),
    };
    json!({
        "luna/State": [&full, &slim],
        "luna/Preferences": [Preferences::default(), slim_preferences],
        "luna/StateReply": [state_reply(&full, Some(true)), state_reply(&slim, None)],
        "luna/GetStateRequest": [GetStateRequest { subscribe: true }],
        "luna/SetEnabledRequest": [SetEnabledRequest { enabled: false }],
        "luna/UninstallRequest": [UninstallRequest {}],
    })
}

/// Two saved color profiles, the first active.
fn with_profiles() -> Settings {
    Settings {
        processing: Processing {
            profiles: vec![
                ColorProfile {
                    id: "ps5".into(),
                    name: "PS5".into(),
                    correction: CorrectionControls {
                        warmth: 12,
                        tint: -4,
                        midtones: 6,
                        intensity: 15,
                        hue: HueControls {
                            red: -5,
                            ..HueControls::default()
                        },
                    },
                },
                ColorProfile {
                    id: "0b6f2c1e-1d2a-4c3b-9f4e-5a6b7c8d9e0f".into(),
                    name: "TV apps".into(),
                    correction: CorrectionControls::default(),
                },
            ],
            active_profile: Some("ps5".into()),
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
            output_issue: Some(OutputIssue::Overridden),
            surface_failures: vec![Error::other("portal listener failed")],
            ..Snapshot::default()
        },
        Snapshot {
            target: LightingTarget::Processed,
            session: SessionState::Running,
            settings: with_profiles(),
            calibration: Some(Calibration {
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
    snapshots.iter().map(State::from).collect()
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
        "Calibration": Pattern::ALL
            .into_iter()
            .enumerate()
            .map(|(index, pattern)| Calibration { pattern, uncorrected: index % 2 == 1 })
            .collect::<Vec<_>>(),
        "SettingsRequest": [
            SettingsRequest { expected: Revision(0), settings: Settings::default() },
            SettingsRequest { expected: Revision(7), settings: configured() },
            SettingsRequest { expected: Revision(8), settings: with_profiles() },
        ],
        "SettingsReplaced": [SettingsReplaced { revision: Revision(8) }],
        "Settings": [Settings::default(), with_profiles()],
        "Candidate": [
            Candidate::new("USB Video", &json!({ "path": "/dev/video0" })).detail("/dev/video0"),
            Candidate::new("Living room", &json!({ "address": "192.168.1.40" }))
                .detail("192.168.1.40, 120 LEDs"),
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
            AdapterSettings::new(
                RemoteReceiver::KIND,
                &RemoteReceiverSettings::new(SocketAddr::from(([0, 0, 0, 0], 7380))),
            ),
            AdapterSettings::new(
                RemoteReceiver::KIND,
                &RemoteReceiverSettings::with_request(
                    SocketAddr::from(([0, 0, 0, 0], 7380)),
                    StreamRequest {
                        width: 640,
                        height: 280,
                        fps: 25,
                        include_ui: false,
                        encoding: StreamEncoding::H264Software,
                    },
                ),
            ),
        ],
        "WledSettings": [AdapterSettings::new(
            Wled::KIND,
            &WledSettings {
                address: "192.0.2.1".into(),
                leds: LedCounts { top: 10, left: 4, ..LedCounts::default() },
            },
        )],
        "WebosSettings": [AdapterSettings::new(
            WebosCapture::KIND,
            &WebosSettings { fps: 60, include_ui: false, ..WebosSettings::default() },
        )],
        "NullSettings": [AdapterSettings::new(NullOutput::KIND, &NullSettings::new())],
    })
}
