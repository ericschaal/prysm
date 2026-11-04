//! Saved preferences and their resolution into a runnable capture target.
use prysm_webos_protocol::{Compression, InstanceUrl, Source, WorkerConfig};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ProcessingLocation {
    #[default]
    Remote,
    Embedded,
}
impl ProcessingLocation {
    #[allow(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde skip_serializing_if takes a reference"
    )]
    fn is_remote(&self) -> bool {
        *self == Self::Remote
    }
}

/// The stored form includes the inactive mode's preferences and may be incomplete.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Config {
    #[serde(default, skip_serializing_if = "ProcessingLocation::is_remote")]
    pub location: ProcessingLocation,
    #[serde(rename = "instanceUrl")]
    #[cfg_attr(feature = "schema", schema(value_type = Option<String>, required = true))]
    pub instance_url: Option<InstanceUrl>,
    pub source: Source,
    pub compression: Compression,
    pub autostart: bool,
    pub metrics: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            location: ProcessingLocation::Remote,
            instance_url: None,
            source: Source::Both,
            compression: Compression::Lz4,
            autostart: false,
            metrics: false,
        }
    }
}

/// Every variant has exactly the data its worker consumes.
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureTarget {
    Remote(WorkerConfig),
    #[cfg(feature = "embedded-prysm")]
    Embedded(prysm_webos_protocol::worker::EmbeddedWorkerConfig),
}

#[derive(Debug, Error)]
pub enum SetupError {
    #[error("Prysm instance URL must be configured")]
    MissingReceiver,
    #[error("Hardware H.264 is not included in this build")]
    UnsupportedEncoder,
    #[error("Embedded Prysm is not included in this build")]
    UnsupportedEmbedded,
}

impl Config {
    /// The TV boot hook activates the service independently of capture eligibility.
    #[must_use]
    pub fn needs_boot_hook(&self) -> bool {
        self.autostart || self.location == ProcessingLocation::Embedded
    }

    pub fn resolve(&self) -> Result<CaptureTarget, SetupError> {
        match self.location {
            ProcessingLocation::Remote => {
                if self.compression == Compression::H264Hardware && !cfg!(feature = "hardware-h264")
                {
                    return Err(SetupError::UnsupportedEncoder);
                }
                self.remote_worker().map(CaptureTarget::Remote)
            }
            #[cfg(feature = "embedded-prysm")]
            ProcessingLocation::Embedded => Ok(CaptureTarget::Embedded(
                prysm_webos_protocol::worker::EmbeddedWorkerConfig {
                    source: self.source,
                    metrics: self.metrics,
                },
            )),
            #[cfg(not(feature = "embedded-prysm"))]
            ProcessingLocation::Embedded => Err(SetupError::UnsupportedEmbedded),
        }
    }
    fn remote_worker(&self) -> Result<WorkerConfig, SetupError> {
        Ok(WorkerConfig {
            instance_url: self
                .instance_url
                .clone()
                .ok_or(SetupError::MissingReceiver)?,
            source: self.source,
            compression: self.compression,
            metrics: self.metrics,
        })
    }

    /// A build without the embedded worker can still load both saved profiles.
    pub fn restore(mut self) -> anyhow::Result<Self> {
        if !cfg!(feature = "embedded-prysm") && self.location == ProcessingLocation::Embedded {
            self.location = ProcessingLocation::Remote;
            if !cfg!(feature = "hardware-h264") && self.compression == Compression::H264Hardware {
                self.compression = Compression::Lz4;
            }
        }
        match self.resolve() {
            Ok(_) | Err(SetupError::MissingReceiver) => Ok(self),
            Err(error) => Err(error.into()),
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

    fn local() -> Config {
        Config {
            location: ProcessingLocation::Embedded,
            source: Source::Video,
            compression: Compression::H264Hardware,
            metrics: true,
            ..Config::default()
        }
    }
    #[test]
    fn embedded_selection_needs_neither_a_receiver_nor_portal_settings() {
        let mut prefs = local();
        #[cfg(feature = "embedded-prysm")]
        {
            let CaptureTarget::Embedded(startup) = prefs.resolve().unwrap() else {
                panic!("wrong target")
            };
            assert_eq!(
                serde_json::to_value(startup).unwrap(),
                serde_json::json!({"source":"video","metrics":true})
            );
        }
        #[cfg(not(feature = "embedded-prysm"))]
        assert!(matches!(
            prefs.resolve(),
            Err(SetupError::UnsupportedEmbedded)
        ));
        prefs.location = ProcessingLocation::Remote;
        prefs.compression = Compression::Lz4;
        assert!(matches!(prefs.resolve(), Err(SetupError::MissingReceiver)));
    }
    #[test]
    fn luna_does_not_accept_portal_settings() {
        let mut input = serde_json::to_value(local()).unwrap();
        input["embeddedOutput"] = serde_json::json!({"type":"wled","url":"http://somewhere/","leds":{"top":10,"right":5,"bottom":10,"left":5}});
        assert!(serde_json::from_value::<Config>(input).is_err());
    }
    #[test]
    fn hardware_compression_requires_the_build_feature() {
        let config = Config {
            compression: Compression::H264Hardware,
            ..valid()
        };
        assert_eq!(config.resolve().is_ok(), cfg!(feature = "hardware-h264"));
    }
    #[test]
    fn configuration_rejects_unknown_fields_addresses_and_shell_input() {
        let value = valid();
        value.resolve().unwrap();
        let mut ipv6 = value.clone();
        ipv6.instance_url = Some("http://[::1]:8787".parse().unwrap());
        ipv6.resolve().unwrap();
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
                Ok(config) => config.resolve().is_err(),
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
                Ok(config) => config.resolve().is_err(),
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
        assert!(!config.remote_worker().unwrap().metrics);
        config.metrics = true;
        assert!(config.remote_worker().unwrap().metrics);
        assert_eq!(
            serde_json::from_value::<Config>(serde_json::to_value(&config).unwrap()).unwrap(),
            config
        );
    }

    #[test]
    fn typed_config_preserves_json_and_worker_settings() {
        let empty = Config::default();
        let json = serde_json::json!({"instanceUrl":null,"source":"both","compression":"lz4","autostart":false,"metrics":false});
        assert_eq!(serde_json::to_value(&empty).unwrap(), json);
        assert_eq!(serde_json::from_value::<Config>(json).unwrap(), empty);
        assert!(empty.remote_worker().is_err());
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
                    config.remote_worker().unwrap(),
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
