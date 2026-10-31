use super::*;
use serde_json::json;
use utoipa::PartialSchema;

const CONFIG: &str = r#"
revision = "0"
source = "prysm"
capture_resolution = [640, 360]
[web]
listen = "127.0.0.1:8787"
allowed_hosts = ["localhost:8787"]
[wled]
url = "http://wled.local/"
[input]
refresh_rate_hz = 30
type = "device"
device = "/dev/video0"
[output]
type = "wled"
address = "wled.local:4048"
[output.leds]
top = 51
right = 31
bottom = 49
left = 31
[processing]
brightness_percent = 75
shadow_lift_percent = 0
smoothing_seconds = 0.1
sample_density = 128
spatial_reach_percent = 10
remove_black_bars = true
"#;

#[test]
fn numeric_values_are_checked_at_deserialization() {
    for invalid in ["-1", "101", "1.5", "\"20\""] {
        assert!(serde_json::from_str::<BrightnessPercent>(invalid).is_err());
        assert!(serde_json::from_str::<ShadowLiftPercent>(invalid).is_err());
    }
    for invalid in ["0", "101", "-1"] {
        assert!(serde_json::from_str::<SpatialReachPercent>(invalid).is_err());
    }
    assert!(serde_json::from_str::<SampleDensity>("0").is_err());
    for invalid in [f32::NAN, f32::INFINITY, -0.1] {
        assert!(SmoothingSeconds::new(invalid).is_err());
    }
    assert!(serde_json::from_str::<ProcessingSettings>("{}").is_err());
    for value in ["wled.local:4048", "127.0.0.1:4048", "[::1]:4048"] {
        assert!(WledAddress::try_from(value.to_owned()).is_ok());
    }
    for value in [
        "",
        "wled.local",
        "wled.local:0",
        "wled.local:4048/path",
        "user@wled.local:4048",
    ] {
        assert!(WledAddress::try_from(value.to_owned()).is_err());
    }
}
#[test]
fn revisions_preserve_u64_precision_and_reject_noncanonical_strings() {
    let revision = ConfigRevision::new(u64::MAX);
    assert_eq!(
        serde_json::to_value(revision).unwrap(),
        json!(u64::MAX.to_string())
    );
    assert!(revision.next().is_err());
    for invalid in [json!(1), json!("01"), json!("+1"), json!("-1"), json!("")] {
        assert!(serde_json::from_value::<ConfigRevision>(invalid).is_err());
    }
}
#[test]
fn one_configuration_round_trips_between_toml_json_and_processor() {
    let config: Config = toml::from_str(CONFIG).unwrap();
    config.validate().unwrap();
    assert_eq!(
        serde_json::from_str::<Config>(&serde_json::to_string(&config).unwrap()).unwrap(),
        config
    );
    assert_eq!(
        toml::from_str::<Config>(&toml::to_string(&config).unwrap()).unwrap(),
        config
    );
    assert_eq!(
        ProcessingSettings::default().to_core(),
        prysm_core::Config::default()
    );
    assert!(
        toml::from_str::<Config>(&CONFIG.replace("brightness_percent = 75", "typo = 75")).is_err()
    );
    assert!(toml::from_str::<Config>(&CONFIG.replace("remove_black_bars = true", "")).is_err());
}
#[test]
fn configuration_rejects_invalid_resources() {
    for invalid in [json!([0, 360]), json!([u32::MAX, u32::MAX]), json!([640])] {
        assert!(serde_json::from_value::<CaptureResolution>(invalid).is_err());
    }
    for invalid in [
        json!({"top":0,"right":0,"bottom":0,"left":0}),
        json!({"top":65535,"right":1,"bottom":0,"left":0}),
    ] {
        assert!(serde_json::from_value::<LedCounts>(invalid).is_err());
    }
    for invalid in [
        "ftp://wled.local/",
        "http://user@wled.local/",
        "http://wled.local/api",
        "http://wled.local/?query=true",
    ] {
        assert!(toml::from_str::<Config>(&CONFIG.replace("http://wled.local/", invalid)).is_err());
    }
    let mut config: Config = toml::from_str(CONFIG).unwrap();
    config.web.allowed_hosts.clear();
    assert!(config.validate().is_err());
}
#[test]
fn webos_input_round_trips() {
    let config: Config = toml::from_str(CONFIG).unwrap();
    let mut next = config.clone();
    next.input = Input::Webos {
        refresh_rate_hz: 30.try_into().unwrap(),
        listen: "0.0.0.0:19400".parse().unwrap(),
    };
    assert_eq!(
        toml::from_str::<Config>(&toml::to_string(&next).unwrap()).unwrap(),
        next
    );
    assert_eq!(
        serde_json::from_value::<Input>(
            json!({"type":"webos", "refresh_rate_hz":30, "listen":"127.0.0.1:19400"})
        )
        .unwrap(),
        Input::Webos {
            refresh_rate_hz: 30.try_into().unwrap(),
            listen: "127.0.0.1:19400".parse().unwrap()
        }
    );
    for invalid in ["tv.local:19400", "0.0.0.0", "bad"] {
        assert!(
            serde_json::from_value::<Input>(
                json!({"type":"webos", "refresh_rate_hz":30, "listen":invalid})
            )
            .is_err()
        );
    }
}
#[test]
fn schema_matches_serialized_values() {
    let schema = |value| serde_json::to_value(value).unwrap();
    let brightness = schema(BrightnessPercent::schema());
    assert_eq!(brightness["type"], "integer");
    assert_eq!(brightness["maximum"], 100.0);
    assert_eq!(schema(ConfigRevision::schema())["type"], "string");
    assert_eq!(schema(LedCounts::schema())["type"], "object");
    assert_eq!(schema(CaptureResolution::schema())["minItems"], 2);
    assert_eq!(
        schema(ProcessingSettings::schema())["required"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    let config = schema(Config::schema());
    assert!(config["properties"].get("processing").is_some());
    assert!(config["properties"].get("state_dir").is_none());
}

#[test]
fn every_input_output_pair_is_valid_without_wled_http_control() {
    let base: Config = toml::from_str(CONFIG).unwrap();
    let inputs = [
        json!({"type":"device", "refresh_rate_hz":30, "device":"/dev/video0"}),
        json!({"type":"file", "refresh_rate_hz":30, "path":"/tmp/video.mkv"}),
        json!({"type":"webos", "refresh_rate_hz":30, "listen":"127.0.0.1:19400"}),
    ];
    let outputs = [
        serde_json::to_value(&base.output).unwrap(),
        json!({"type":"egui"}),
    ];
    for input in inputs {
        for output in &outputs {
            let mut config = base.clone();
            config.input = serde_json::from_value(input.clone()).unwrap();
            config.output = serde_json::from_value(output.clone()).unwrap();
            config.wled = None;
            config.validate().unwrap();
            assert_eq!(
                toml::from_str::<Config>(&toml::to_string(&config).unwrap()).unwrap(),
                config
            );
            config.source = Source::Wled;
            assert!(config.validate().is_err());
        }
    }
}

#[test]
fn input_rates_are_required_positive_and_output_rates_are_rejected() {
    for input in [
        json!({"type":"device", "device":"/dev/video0"}),
        json!({"type":"file", "path":"video.mkv"}),
        json!({"type":"webos", "listen":"127.0.0.1:19400"}),
    ] {
        assert!(serde_json::from_value::<Input>(input.clone()).is_err());
        for rate in [json!(0), json!(-1), json!(1.5), json!("30")] {
            let mut invalid = input.clone();
            invalid["refresh_rate_hz"] = rate;
            assert!(serde_json::from_value::<Input>(invalid).is_err());
        }
        let mut valid = input;
        valid["refresh_rate_hz"] = json!(30);
        assert!(serde_json::from_value::<Input>(valid).is_ok());
    }
    let base: Config = toml::from_str(CONFIG).unwrap();
    for mut output in [
        serde_json::to_value(base.output).unwrap(),
        json!({"type":"egui"}),
    ] {
        output["refresh_rate_hz"] = json!(30);
        assert!(serde_json::from_value::<Output>(output).is_err());
    }
}

#[test]
fn webos_capture_rate_cannot_exceed_native_limit() {
    let mut config: Config = toml::from_str(CONFIG).unwrap();
    for rate in [1, 60, 61] {
        config.input = Input::Webos {
            listen: "127.0.0.1:19400".parse().unwrap(),
            refresh_rate_hz: rate.try_into().unwrap(),
        };
        assert_eq!(config.validate().is_ok(), rate <= 60);
    }
}
