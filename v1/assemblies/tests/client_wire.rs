#![cfg(feature = "wire")]

use prysm_protocol::{
    application::{Cursor, InstanceId, LightingTarget, OperationId, SettingsRevision},
    client::ClientSnapshot,
};
use std::num::NonZeroU64;

#[test]
fn client_ids_preserve_all_bits_and_reject_invalid_decimal_encodings() {
    let domain_id = prysm_contract::identity::OperationId {
        instance: prysm_contract::identity::InstanceId(NonZeroU64::new(u64::MAX).unwrap()),
        request: NonZeroU64::new(u64::MAX).unwrap(),
    };
    let id = OperationId::from(domain_id);
    let json = serde_json::to_value(id).unwrap();
    assert_eq!(json["instance"], u64::MAX.to_string());
    assert_eq!(json["request"], u64::MAX.to_string());
    assert_eq!(serde_json::from_value::<OperationId>(json).unwrap(), id);
    assert_eq!(prysm_contract::identity::OperationId::from(id), domain_id);
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
fn operation_failure_serializes_its_category_without_local_error_types() {
    use prysm_protocol::application::OperationStatus;
    let failure = prysm_contract::Failure::from(prysm_contract::ControlError::RecoveryRequired(
        Box::new(std::io::Error::other("ambiguous device response")),
    ));
    let status = OperationStatus::from(prysm_contract::application::OperationStatus::Failed(
        failure,
    ));
    let json = serde_json::to_value(status).unwrap();
    assert_eq!(json["kind"], "failed");
    assert_eq!(json["failure"]["kind"], "recovery_required");
    assert_eq!(json["failure"].as_object().unwrap().len(), 2);
    assert!(
        json["failure"]["message"]
            .as_str()
            .unwrap()
            .contains("ambiguous device response")
    );
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
