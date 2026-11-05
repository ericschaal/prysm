#![cfg(feature = "wire")]

use prysm_assemblies::{
    DesktopCaptureSettings, RemoteCaptureSettings, ServerCaptureSettings, TvCaptureSettings,
    WledSettings,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn roundtrip<T: Serialize + DeserializeOwned>(value: Value) {
    let decoded: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[test]
fn adapter_settings_roundtrip_the_documented_wire_shapes() {
    for source in ["device", "file"] {
        let value =
            json!({"source": source, "path": "fixture", "rate": 30, "resolution": [320, 180]});
        roundtrip::<DesktopCaptureSettings>(value.clone());
        roundtrip::<ServerCaptureSettings>(json!({"adapter": "desktop", "settings": value}));
    }
    let remote = json!({"peer": "127.0.0.1:9000"});
    roundtrip::<RemoteCaptureSettings>(remote.clone());
    roundtrip::<ServerCaptureSettings>(json!({"adapter": "remote", "settings": remote}));
    roundtrip::<TvCaptureSettings>(json!({"rate": 30, "resolution": [320, 180]}));
    roundtrip::<WledSettings>(json!({"address": "192.0.2.1", "leds": [1, 2, 3, 4]}));
}

#[test]
fn decoding_keeps_domain_validation() {
    for resolution in [
        json!([0, 180]),
        json!([3841, 180]),
        json!([320, 2161]),
        json!([320]),
    ] {
        assert!(
            serde_json::from_value::<TvCaptureSettings>(
                json!({"rate": 30, "resolution": resolution})
            )
            .is_err()
        );
    }
    assert!(
        serde_json::from_value::<TvCaptureSettings>(json!({"rate": 0, "resolution": [320, 180]}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<DesktopCaptureSettings>(
            json!({"source": "remote", "peer": "127.0.0.1:9000", "rate": 30, "resolution": [320, 180]})
        )
        .is_err()
    );
    for value in [
        json!({"peer": "bad address"}),
        json!({"peer": "127.0.0.1:9000", "resolution": [320, 180], "rate": 30}),
    ] {
        assert!(serde_json::from_value::<RemoteCaptureSettings>(value).is_err());
    }
    assert!(serde_json::from_value::<ServerCaptureSettings>(json!({
        "adapter": "remote", "settings": {"source": "device", "path": "fixture", "rate": 30, "resolution": [320, 180]}
    })).is_err());
    for value in [
        json!({"address": "bad address", "leds": [1, 2, 3, 4]}),
        json!({"address": "192.0.2.1", "leds": [1, 2, 3]}),
        json!({"address": "192.0.2.1", "leds": [1, 2, 3, 4294967296_u64]}),
    ] {
        assert!(serde_json::from_value::<WledSettings>(value).is_err());
    }
}
