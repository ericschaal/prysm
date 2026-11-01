use anyhow::{Result, bail};
use serde_json::{Value, json};

#[derive(Default)]
pub struct Status {
    power: Option<String>,
    error: Option<String>,
    revision: u64,
}

impl Status {
    pub fn observe_power(&mut self, value: &Value) -> bool {
        let (power, error) = if value["returnValue"] == false || !value["state"].is_string() {
            (None, Some(format!("TV power subscription failed: {value}")))
        } else {
            (value["state"].as_str().map(str::to_owned), None)
        };
        if self.power == power && self.error == error {
            return false;
        }
        self.power = power;
        self.error = error;
        self.revision += 1;
        true
    }

    pub fn power(&self) -> Option<String> {
        self.power.clone()
    }
    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }
    pub fn bump(&mut self) {
        self.revision += 1;
    }
    pub fn snapshot(&self, subscribed: bool, subscribers: u32) -> Value {
        json!({
            "returnValue": true,
            "subscribed": subscribed,
            "power": self.power,
            "error": self.error,
            "revision": self.revision,
            "subscribers": subscribers,
            "pid": std::process::id(),
        })
    }
}

pub fn subscription_request(value: &Value) -> Result<bool> {
    let Some(fields) = value.as_object() else {
        bail!("status requires a JSON object");
    };
    if fields.keys().any(|key| key != "subscribe") {
        bail!("unknown status parameter");
    }
    match value.get("subscribe") {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => bail!("subscribe must be a boolean"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_updates_only_publish_changes_and_fail_closed_on_errors() {
        let mut status = Status::default();
        assert!(status.snapshot(false, 0)["power"].is_null());
        assert!(status.observe_power(&json!({"returnValue": true, "state": "Active"})));
        assert!(!status.observe_power(&json!({"state": "Active"})));
        assert_eq!(status.snapshot(true, 1)["revision"], 1);
        assert!(status.observe_power(&json!({"state": "Suspend"})));
        assert!(status.observe_power(&json!({"returnValue": false, "errorText": "denied"})));
        assert!(status.snapshot(false, 0)["power"].is_null());
        assert!(status.snapshot(false, 0)["error"].is_string());
        assert!(status.observe_power(&json!({"state": "Active"})));
        assert!(status.snapshot(false, 0)["error"].is_null());
    }

    #[test]
    fn status_accepts_only_an_optional_boolean_subscription() {
        assert!(!subscription_request(&json!({})).unwrap());
        assert!(subscription_request(&json!({"subscribe": true})).unwrap());
        for invalid in [
            json!(null),
            json!([]),
            json!({"subscribe": 1}),
            json!({"command": "id"}),
        ] {
            assert!(subscription_request(&invalid).is_err());
        }
    }
}
