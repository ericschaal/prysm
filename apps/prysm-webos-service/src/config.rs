//! Saved service configuration and conversion to the worker startup contract.
use anyhow::{Context, Result};
use prysm_webos_protocol::{Compression, InstanceUrl, Source, WorkerConfig};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Config {
    #[serde(with = "instance_url", rename = "instanceUrl")]
    #[cfg_attr(feature = "schema", schema(value_type = String, required = true))]
    pub instance_url: Option<InstanceUrl>,
    pub source: Source,
    pub compression: Compression,
    pub autostart: bool,
    pub metrics: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            instance_url: None,
            source: Source::Both,
            compression: Compression::Lz4,
            autostart: false,
            metrics: false,
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        self.instance_url
            .as_ref()
            .context("Prysm instance URL must be configured")?;
        anyhow::ensure!(
            self.compression != Compression::H264Hardware || cfg!(feature = "hardware-h264"),
            "Hardware H.264 is not included in this build"
        );
        Ok(())
    }
    pub fn worker_config(&self) -> Result<WorkerConfig> {
        Ok(WorkerConfig {
            instance_url: self
                .instance_url
                .clone()
                .context("Prysm instance URL must be configured")?,
            source: self.source,
            compression: self.compression,
            metrics: self.metrics,
        })
    }
}

// The TV represents an unconfigured instance with an empty string.
mod instance_url {
    use super::InstanceUrl;
    use serde::{Deserialize, Deserializer, Serializer};

    #[allow(clippy::ref_option, reason = "Serde passes a reference to the field")]
    pub fn serialize<S: Serializer>(
        address: &Option<InstanceUrl>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match address {
            Some(address) => serializer.collect_str(address),
            None => serializer.serialize_str(""),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<InstanceUrl>, D::Error> {
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
            instance_url: Some("http://127.0.0.1:8787".parse().unwrap()),
            ..Config::default()
        }
    }

    #[test]
    fn hardware_compression_requires_the_build_feature() {
        let config = Config {
            compression: Compression::H264Hardware,
            ..valid()
        };
        assert_eq!(config.validate().is_ok(), cfg!(feature = "hardware-h264"));
    }
    #[test]
    fn configuration_rejects_unknown_fields_addresses_and_shell_input() {
        let value = valid();
        value.validate().unwrap();
        let mut ipv6 = value.clone();
        ipv6.instance_url = Some("http://[::1]:8787".parse().unwrap());
        ipv6.validate().unwrap();
        for address in [
            "",
            "ftp://example.com",
            "1.2.3.4:0",
            "1.2.3.4:65536",
            "::1:19400",
            "1.2.3.4:1;reboot",
        ] {
            let mut json = serde_json::to_value(&value).unwrap();
            json["instanceUrl"] = address.into();
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
    fn metrics_are_required_and_reach_the_worker() {
        let mut config = valid();
        let mut incomplete = serde_json::to_value(&config).unwrap();
        incomplete.as_object_mut().unwrap().remove("metrics");
        assert!(serde_json::from_value::<Config>(incomplete).is_err());
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
        let json = serde_json::json!({"instanceUrl":"","source":"both","compression":"lz4","autostart":false,"metrics":false});
        assert_eq!(serde_json::to_value(&empty).unwrap(), json);
        assert_eq!(serde_json::from_value::<Config>(json).unwrap(), empty);
        assert!(empty.worker_config().is_err());
        for (source, source_name) in [(Source::Video, "video"), (Source::Both, "both")] {
            for (compression, compression_name) in [
                (Compression::None, "none"),
                (Compression::Lz4, "lz4"),
                (Compression::H264Software, "h264-software"),
                (Compression::H264Hardware, "h264-hardware"),
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
                        instance_url: "http://127.0.0.1:8787".parse().unwrap(),
                        source,
                        compression,
                        metrics: false,
                    }
                );
            }
        }
    }
}
