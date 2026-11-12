//! Actual serializations of every wire root, validated against the generated
//! schemas by the frontend contract tests.
use prysm_capture_device::{DeviceSettings, DeviceSource};
use prysm_capture_remote::{RemoteReceiver, RemoteReceiverSettings};
use prysm_capture_webos::{WebosCapture, WebosSettings};
use prysm_output_wled::{LedCounts, Wled, WledSettings};
use prysm_ports::{
    AdapterSettings, Error, LightingTarget, Processing, Revision, SessionPhase, SessionState,
    Settings, Snapshot, ValidationError,
};
use prysm_ports::{OutputFactory, SourceFactory};
use prysm_protocol::{Failure, SettingsReplaced, SettingsRequest, State, TargetRequest};
use prysm_runtime::{NullOutput, NullSettings};
use prysm_webos_link::{SlimCapture, Video};
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
    let mut fixtures = application();
    fixtures
        .as_object_mut()
        .expect("fixtures are an object")
        .extend(luna().as_object().expect("fixtures are an object").clone());
    fixtures
}

/// The TV supervisor's Luna values, one representative state per flavor.
fn luna() -> Value {
    let slim_preferences = Preferences {
        autostart: true,
        instance: Some("http://192.168.1.20:8787".into()),
        receiver: Some(SocketAddr::from(([192, 168, 1, 20], 7380))),
        capture: SlimCapture {
            fps: 25,
            include_ui: false,
            ..SlimCapture::default()
        },
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
        portal: Some("http://192.168.1.5:8787/".into()),
        failure: None,
        log: vec!["prysm-webos: portal on 0.0.0.0:8787".into()],
    };
    let slim = TvState {
        flavor: Flavor::Slim,
        preferences: slim_preferences.clone(),
        enabled: true,
        lighting: None,
        power: Power::ScreenSaver,
        video: Video::Unknown,
        worker: Phase::Retrying,
        capturing: false,
        portal: None,
        failure: Some("connect 192.168.1.20:7380: connection refused".into()),
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

/// The portal's values and the host documents' adapter sections.
fn application() -> Value {
    let configured = Settings {
        capture: Some(AdapterSettings::new(
            "device",
            &json!({ "path": "/dev/video0" }),
        )),
        output: Some(AdapterSettings::new(
            "wled",
            &json!({ "address": "192.0.2.1", "leds": { "top": 10, "left": 4 } }),
        )),
        processing: Processing::default(),
    };
    let states = [
        Snapshot::default(),
        Snapshot {
            target: LightingTarget::Processed,
            session: SessionState::Running,
            settings: configured.clone(),
            revision: Revision(7),
            surface_failures: vec![Error::other("portal listener failed")],
            ..Snapshot::default()
        },
        Snapshot {
            target: LightingTarget::DeviceNative,
            session: SessionState::Native,
            ..Snapshot::default()
        },
        Snapshot {
            target: LightingTarget::Off,
            session: SessionState::Faulted(SessionPhase::IdlingOutput),
            failure: Some(Error::Faulted(SessionPhase::IdlingOutput)),
            ..Snapshot::default()
        },
    ];
    let failures = [
        Error::from(ValidationError::new(
            "output.address",
            "a unicast device address is required",
        )),
        Error::StaleRevision {
            expected: Revision(1),
            current: Revision(2),
        },
        Error::other("device unplugged"),
        Error::Closed,
    ];
    json!({
        "State": states.iter().map(State::from).collect::<Vec<_>>(),
        "Failure": failures.iter().map(Failure::from).collect::<Vec<_>>(),
        "TargetRequest": [TargetRequest { target: LightingTarget::Processed }],
        "SettingsRequest": [
            SettingsRequest { expected: Revision(0), settings: Settings::default() },
            SettingsRequest { expected: Revision(7), settings: configured },
        ],
        "SettingsReplaced": [SettingsReplaced { revision: Revision(8) }],
        "Settings": [Settings::default()],
        // Host-document sections, as the registry stores them.
        "DeviceSettings": [AdapterSettings::new(
            DeviceSource::KIND,
            &DeviceSettings::new("/dev/video0"),
        )],
        "RemoteReceiverSettings": [AdapterSettings::new(
            RemoteReceiver::KIND,
            &RemoteReceiverSettings { listen: SocketAddr::from(([0, 0, 0, 0], 7380)) },
        )],
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
