use prysm_webos_service::contract::{ErrorReply, ServiceStatus};
use std::{error::Error, io::Write};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(components(schemas(ServiceStatus, ErrorReply)))]
struct Schema;

fn main() -> Result<(), Box<dyn Error>> {
    std::io::stdout()
        .lock()
        .write_all(Schema::openapi().to_pretty_json()?.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_webos_service::{
        config::Config,
        contract::{Capabilities, Observation, SupervisorStatus},
    };
    use serde_json::json;

    #[test]
    fn status_schema_and_serialization_preserve_the_luna_contract() {
        let status = ServiceStatus {
            observation: Observation {
                return_value: true,
                subscribed: true,
                video_available: None,
                revision: 3,
                subscribers: 1,
                pid: 42,
            },
            supervisor: SupervisorStatus {
                config: Config::default(),
                enabled: false,
                receiver_connected: false,
                capturing: false,
                sender_running: false,
                stopping: false,
                power: None,
                error: None,
                last_log: None,
                capabilities: Capabilities {
                    hardware_h264: false,
                },
            },
        };
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(
            value,
            json!({
                "returnValue":true, "subscribed":true, "videoAvailable":null, "revision":3, "subscribers":1, "pid":42,
                "config":{"receiver":"", "source":"both", "compression":"lz4", "autostart":false, "metrics":false},
                "enabled":false, "receiverConnected":false, "capturing":false, "senderRunning":false,
                "stopping":false, "power":null, "error":null, "lastLog":null, "capabilities":{"hardwareH264":false}
            })
        );
        serde_json::from_value::<ServiceStatus>(value).unwrap();
        let schema = serde_json::to_value(Schema::openapi()).unwrap();
        let config = &schema["components"]["schemas"]["Config"];
        assert_eq!(config["properties"]["receiver"]["type"], "string");
        assert!(
            config["required"]
                .as_array()
                .unwrap()
                .contains(&json!("metrics"))
        );
        assert_eq!(
            serde_json::to_value(ErrorReply::new("failed".into())).unwrap(),
            json!({"returnValue":false, "errorText":"failed"})
        );
    }
}
