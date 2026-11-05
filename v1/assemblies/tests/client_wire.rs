#![cfg(feature = "wire")]

use prysm_contract::{
    application::LightingTarget,
    client::ClientSnapshot,
    identity::{InstanceId, OperationId, SettingsRevision},
    observation::Cursor,
};
use std::num::NonZeroU64;

#[test]
fn client_ids_preserve_all_bits_and_reject_invalid_decimal_encodings() {
    let id = OperationId {
        instance: InstanceId(NonZeroU64::new(u64::MAX).unwrap()),
        request: NonZeroU64::new(u64::MAX).unwrap(),
    };
    let json = serde_json::to_value(id).unwrap();
    assert_eq!(json["instance"], u64::MAX.to_string());
    assert_eq!(json["request"], u64::MAX.to_string());
    assert_eq!(serde_json::from_value::<OperationId>(json).unwrap(), id);
    for value in [
        r#""0""#,
        r#""01""#,
        r#""-1""#,
        r#""18446744073709551616""#,
        "1",
    ] {
        assert!(serde_json::from_str::<InstanceId>(value).is_err());
    }
}

#[test]
fn client_snapshot_has_no_private_calibration_payload() {
    let view = ClientSnapshot::<(), ()> {
        cursor: Cursor {
            instance: InstanceId(NonZeroU64::new(1).unwrap()),
            sequence: 0,
        },
        revision: SettingsRevision(0),
        capture: None,
        output: None,
        target: LightingTarget::Off,
    };
    let json = serde_json::to_value(view).unwrap();
    assert_eq!(json["revision"], "0");
    assert!(json["capture"].is_null());
    assert_eq!(json.as_object().unwrap().len(), 5);
    for private in ["pairing", "session", "token", "calibration"] {
        assert!(json.get(private).is_none());
    }
}
