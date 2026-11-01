//! Saved service configuration and conversion to the worker startup contract.
use anyhow::{Context, Result};
use prysm_webos_protocol::{Compression, ReceiverAddress, Source, WorkerConfig};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(with = "receiver")]
    pub receiver: Option<ReceiverAddress>,
    pub source: Source,
    pub compression: Compression,
    pub autostart: bool,
    #[serde(default)]
    pub metrics: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            receiver: None,
            source: Source::Both,
            compression: Compression::Lz4,
            autostart: false,
            metrics: false,
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        self.receiver.context("Receiver must be configured")?;
        Ok(())
    }
    pub fn worker_config(&self) -> Result<WorkerConfig> {
        Ok(WorkerConfig {
            receiver: self.receiver.context("Receiver must be configured")?,
            source: self.source,
            compression: self.compression,
            metrics: self.metrics,
        })
    }
}

// Preserve the app's empty-string representation of an unconfigured receiver.
mod receiver {
    use super::ReceiverAddress;
    use serde::{Deserialize, Deserializer, Serializer};

    #[allow(clippy::ref_option, reason = "Serde passes a reference to the field")]
    pub fn serialize<S: Serializer>(
        address: &Option<ReceiverAddress>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match address {
            Some(address) => serializer.collect_str(address),
            None => serializer.serialize_str(""),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<ReceiverAddress>, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty() {
            Ok(None)
        } else {
            value.parse().map(Some).map_err(serde::de::Error::custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid() -> Config {
        Config {
            receiver: Some("127.0.0.1:19400".parse().unwrap()),
            ..Config::default()
        }
    }
    #[test]
    fn configuration_rejects_unknown_fields_addresses_and_shell_input() {
        let value = valid();
        value.validate().unwrap();
        let mut ipv6 = value.clone();
        ipv6.receiver = Some("[::1]:19400".parse().unwrap());
        ipv6.validate().unwrap();
        for address in [
            "",
            "example.com:19400",
            "1.2.3.4:0",
            "1.2.3.4:65536",
            "::1:19400",
            "1.2.3.4:1;reboot",
        ] {
            let mut json = serde_json::to_value(&value).unwrap();
            json["receiver"] = address.into();
            assert!(match serde_json::from_value::<Config>(json) {
                Ok(config) => config.validate().is_err(),
                Err(_) => true,
            });
        }
        for (key, invalid) in [
            ("source", serde_json::json!(";reboot")),
            ("compression", serde_json::json!("gzip")),
            ("autostart", serde_json::json!("true")),
            ("metrics", serde_json::json!("true")),
        ] {
            let mut json = serde_json::to_value(&value).unwrap();
            json[key] = invalid;
            assert!(match serde_json::from_value::<Config>(json) {
                Ok(config) => config.validate().is_err(),
                Err(_) => true,
            });
        }
        let mut json = serde_json::to_value(&value).unwrap();
        json["args"] = serde_json::json!(["--force-capture"]);
        assert!(serde_json::from_value::<Config>(json).is_err());
    }
    #[test]
    fn metrics_default_off_for_saved_configs_and_reach_the_worker_when_enabled() {
        let mut json = serde_json::to_value(valid()).unwrap();
        json.as_object_mut().unwrap().remove("metrics");
        let mut config: Config = serde_json::from_value(json).unwrap();
        assert!(!config.metrics);
        assert!(!config.worker_config().unwrap().metrics);
        config.metrics = true;
        assert!(config.worker_config().unwrap().metrics);
        assert_eq!(
            serde_json::from_value::<Config>(serde_json::to_value(&config).unwrap()).unwrap(),
            config
        );
    }

    #[test]
    fn typed_config_preserves_json_and_worker_settings() {
        let empty = Config::default();
        let json = serde_json::json!({"receiver":"","source":"both","compression":"lz4","autostart":false,"metrics":false});
        assert_eq!(serde_json::to_value(&empty).unwrap(), json);
        assert_eq!(serde_json::from_value::<Config>(json).unwrap(), empty);
        assert!(empty.worker_config().is_err());
        for (source, source_name) in [
            (Source::Video, "video"),
            (Source::Ui, "ui"),
            (Source::Both, "both"),
        ] {
            for (compression, compression_name) in [
                (Compression::None, "none"),
                (Compression::Lz4, "lz4"),
                (Compression::H264, "h264"),
            ] {
                let config = Config {
                    source,
                    compression,
                    ..valid()
                };
                let json = serde_json::to_value(&config).unwrap();
                assert_eq!(json["source"], source_name);
                assert_eq!(json["compression"], compression_name);
                assert_eq!(serde_json::from_value::<Config>(json).unwrap(), config);
                assert_eq!(
                    config.worker_config().unwrap(),
                    WorkerConfig {
                        receiver: "127.0.0.1:19400".parse().unwrap(),
                        source,
                        compression,
                        metrics: false,
                    }
                );
            }
        }
    }
}
