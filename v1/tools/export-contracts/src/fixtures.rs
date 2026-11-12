//! Actual serializations of every wire root, validated against the generated
//! schemas by the frontend contract tests.
use prysm_capture_device::{DeviceSettings, DeviceSource};
use prysm_capture_remote::{RemoteReceiver, RemoteReceiverSettings};
use prysm_output_wled::{LedCounts, Wled, WledSettings};
use prysm_ports::{
    AdapterSettings, Error, LightingTarget, Processing, Revision, SessionPhase, SessionState,
    Settings, Snapshot, ValidationError,
};
use prysm_ports::{OutputFactory, SourceFactory};
use prysm_protocol::{Failure, SettingsReplaced, SettingsRequest, State, TargetRequest};
use prysm_runtime::{NullOutput, NullSettings};
use serde_json::{Value, json};
use std::net::{IpAddr, SocketAddr};

pub fn serialized() -> Value {
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
                address: IpAddr::from([192, 0, 2, 1]),
                leds: LedCounts { top: 10, left: 4, ..LedCounts::default() },
            },
        )],
        "NullSettings": [AdapterSettings::new(NullOutput::KIND, &NullSettings::new())],
    })
}
