//! Typed readings of the two Luna subscriptions the supervisor depends on.
use crate::state::Power;
use prysm_webos_link::Video;
use serde_json::Value;

/// `com.webos.service.tvpower/power/getPowerState`. `None`: an error reply
/// or no `state` (fail closed: callers treat it as [`Power::Unknown`]).
#[must_use]
pub fn power(payload: &Value) -> Option<Power> {
    if payload["returnValue"] == false {
        return None;
    }
    Some(match payload["state"].as_str()? {
        "Active" => Power::Active,
        "Screen Saver" => Power::ScreenSaver,
        _ => Power::Asleep,
    })
}

/// What `com.webos.service.videooutput/getStatus` says about the MAIN sink.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoReading {
    pub video: Video,
    /// Source, port and context of available video; a change resets capture.
    pub context: Option<Value>,
    /// A good subscription reply: stop retrying the subscription.
    pub settled: bool,
}

#[must_use]
pub fn video(payload: &Value) -> VideoReading {
    // No app or port filter: MAIN may carry any input or a built-in video app.
    let main = payload["video"]
        .as_array()
        .and_then(|sinks| sinks.iter().find(|sink| sink["sink"] == "MAIN"))
        .filter(|_| payload["returnValue"] == true);
    let available = main.and_then(|main| {
        if !main["connected"].as_bool()? {
            return Some(false);
        }
        // Muted video can be legitimate black; it still counts.
        Some(
            main["width"].as_u64()? > 0
                && main["height"].as_u64()? > 0
                && main["frameRate"].as_f64()? > 0.0,
        )
    });
    let video = match available {
        Some(true) => Video::Available,
        Some(false) => Video::Unavailable,
        None => Video::Unknown,
    };
    let context = main.filter(|_| video == Video::Available).map(|main| {
        serde_json::json!([
            main["connectedSource"],
            main["connectedSourcePort"],
            main["context"]
        ])
    });
    VideoReading {
        video,
        context,
        settled: video != Video::Unknown && payload["subscribed"] == true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn power_states_fail_closed() {
        for (payload, expected) in [
            (
                json!({"returnValue": true, "state": "Active"}),
                Some(Power::Active),
            ),
            (json!({"state": "Screen Saver"}), Some(Power::ScreenSaver)),
            (json!({"state": "Active Standby"}), Some(Power::Asleep)),
            (json!({"state": "Suspend"}), Some(Power::Asleep)),
            (json!({"returnValue": false, "errorText": "denied"}), None),
            (json!({"returnValue": true}), None),
            (json!({"state": 1}), None),
        ] {
            assert_eq!(power(&payload), expected, "{payload}");
        }
    }

    fn main_sink(fields: Value) -> Value {
        let mut sink = json!({"sink": "MAIN", "connected": true, "width": 1920, "height": 1080,
            "frameRate": 60.0, "connectedSource": "HDMI", "connectedSourcePort": 2, "context": "pipeline1"});
        for (key, value) in fields.as_object().unwrap() {
            sink[key] = value.clone();
        }
        json!({"returnValue": true, "subscribed": true, "video": [{"sink": "SUB"}, sink]})
    }

    #[test]
    fn video_reads_the_main_sink() {
        let reading = video(&main_sink(json!({})));
        assert_eq!(reading.video, Video::Available);
        assert!(reading.settled);
        assert_eq!(reading.context, Some(json!(["HDMI", 2, "pipeline1"])));
        assert_ne!(
            video(&main_sink(json!({"connectedSourcePort": 3}))).context,
            reading.context
        );

        let gone = video(&main_sink(json!({"connected": false})));
        assert_eq!((gone.video, gone.context), (Video::Unavailable, None));
        assert_eq!(
            video(&main_sink(json!({"width": 0}))).video,
            Video::Unavailable
        );
        assert_eq!(
            video(&main_sink(json!({"frameRate": null}))).video,
            Video::Unknown
        );

        let failed = video(&json!({"returnValue": false}));
        assert_eq!(failed.video, Video::Unknown);
        assert!(!failed.settled);
        let unsubscribed =
            video(&json!({"returnValue": true, "video": main_sink(json!({}))["video"]}));
        assert_eq!(unsubscribed.video, Video::Available);
        assert!(!unsubscribed.settled, "keep retrying until subscribed");
    }
}
