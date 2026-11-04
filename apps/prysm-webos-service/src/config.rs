//! Saved preferences and their resolution into a runnable capture target.
use prysm_contract::Output;
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
    #[serde(
        default,
        rename = "embeddedOutput",
        skip_serializing_if = "Option::is_none"
    )]
    pub embedded_output: Option<Output>,
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
            embedded_output: None,
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
    #[error("Configure the WLED address and LED layout")]
    MissingOutput,
    #[error("Invalid WLED output: {0}")]
    InvalidOutput(#[source] anyhow::Error),
}

impl Config {
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
            ProcessingLocation::Embedded => {
                let output = self
                    .embedded_output
                    .as_ref()
                    .ok_or(SetupError::MissingOutput)?;
                output.validate().map_err(SetupError::InvalidOutput)?;
                Ok(CaptureTarget::Embedded(
                    prysm_webos_protocol::worker::EmbeddedWorkerConfig {
                        output: output.clone(),
                        source: self.source,
                        metrics: self.metrics,
                    },
                ))
            }
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
            Ok(_) | Err(SetupError::MissingReceiver | SetupError::MissingOutput) => Ok(self),
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
        serde_json::from_value(serde_json::json!({
            "location":"embedded", "instanceUrl": null, "source":"video", "compression":"h264-hardware",
            "autostart":true, "metrics":true,
            "embeddedOutput":{"type":"wled","url":"http://127.0.0.1/",
                "leds":{"top":51,"right":31,"bottom":49,"left":31}}
        })).unwrap()
    }
    #[test]
    fn selected_mode_is_the_only_source_of_worker_settings() {
        let mut prefs = local();
        #[cfg(feature = "embedded-prysm")]
        {
            let CaptureTarget::Embedded(startup) = prefs.resolve().unwrap() else {
                panic!("wrong target")
            };
            assert_eq!(startup.output, prefs.embedded_output.clone().unwrap());
            assert_eq!(startup.source, Source::Video);
            assert!(startup.metrics);
            let wire = serde_json::to_value(startup).unwrap();
            assert!(wire.get("compression").is_none() && wire.get("instanceUrl").is_none());
            prefs.embedded_output = None;
            assert!(matches!(prefs.resolve(), Err(SetupError::MissingOutput)));
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
    fn embedded_output_validates_url_and_four_edge_counts_at_the_wire_boundary() {
        for leds in [
            serde_json::json!({"top":0,"right":0,"bottom":0,"left":0}),
            serde_json::json!({"top":65535,"right":1,"bottom":0,"left":0}),
            serde_json::json!({"top":1,"right":-1,"bottom":0,"left":0}),
        ] {
            let mut wire = serde_json::to_value(local()).unwrap();
            wire["embeddedOutput"]["leds"] = leds;
            assert!(serde_json::from_value::<Config>(wire).is_err());
        }
        let mut wire = serde_json::to_value(local()).unwrap();
        wire["embeddedOutput"]["url"] = serde_json::json!("file:///tmp/target");
        assert!(serde_json::from_value::<Config>(wire).is_err());
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
