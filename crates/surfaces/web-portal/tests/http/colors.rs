//! The colors stream: what the LEDs show, its cadence and its viewers.
use crate::common::*;
use futures::StreamExt;
use prysm_ports::{AdapterSettings, Processing, Settings, Style, StyleValues};
use prysm_runtime::fakes::{self, FakeSourceSettings};
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::timeout;

async fn colors_stream(portal: &Portal) -> reqwest::Response {
    portal
        .request(Method::GET, "/api/v1/colors/stream")
        .send()
        .await
        .unwrap()
}

fn assert_summary(event: &Value) {
    assert_eq!(event["type"], "colors");
    for edge in ["top", "right", "bottom", "left"] {
        let colors = event[edge].as_array().unwrap();
        assert!((1..=8).contains(&colors.len()), "{edge}: {colors:?}");
        assert_eq!(colors[0].as_array().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn colors_are_idle_unless_lights_follow_the_screen_and_sent_only_when_changed() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let response = colors_stream(&portal).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        assert_secure(&response);
        assert_eq!(portal.engine.colors_viewers(), 1);
        // Listening to colors is not watching the preview: no frame is copied
        // or encoded for it.
        assert_eq!(portal.engine.preview_viewers(), 0);
        let mut events = response.bytes_stream();
        let mut buffer = String::new();
        let first = next_state(&mut events, &mut buffer, |_| true).await;
        assert_eq!(first, json!({ "type": "idle" }));

        assert_eq!(portal.target("screen_sync").await.0, StatusCode::NO_CONTENT);
        let colors = next_state(&mut events, &mut buffer, |_| true).await;
        assert_summary(&colors);
        assert!(colors.get("jpeg").is_none());
        // The fake's frames are all black: the colors settle, then nothing
        // more is sent while they stay the same.
        while let Ok(event) = timeout(
            Duration::from_millis(600),
            next_state(&mut events, &mut buffer, |_| true),
        )
        .await
        {
            assert_eq!(event["type"], "colors");
        }

        // The device's own effects are not Prysm's colors.
        assert_eq!(portal.target("effects").await.0, StatusCode::NO_CONTENT);
        let idle = next_state(&mut events, &mut buffer, |_| true).await;
        assert_eq!(idle, json!({ "type": "idle" }));

        assert_eq!(portal.target("screen_sync").await.0, StatusCode::NO_CONTENT);
        assert_summary(&next_state(&mut events, &mut buffer, |_| true).await);
        assert_eq!(portal.target("off").await.0, StatusCode::NO_CONTENT);
        next_state(&mut events, &mut buffer, |e| e["type"] == "idle").await;

        drop(events);
        eventually_async("no colors viewers", LIMIT, || {
            (portal.engine.colors_viewers() == 0).then_some(())
        })
        .await;
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn colors_cadence_is_capped_at_ten_per_second() {
    within_async(LIMIT, async {
        // Black and white alternate every 25 frames of 10 ms, eased in so the
        // colors keep changing. Taken at once, as a cut, they would alternate
        // in step with the cadence and every other look would show the same
        // color. 10 ms is long against a sleep's lateness, so the phases do
        // not stretch much on a loaded machine.
        let defaults = fakes::settings();
        let eased = StyleValues {
            cut_response_percent: 0,
            ..StyleValues::DEFAULT
        };
        let blinking = Settings {
            capture: Some(AdapterSettings::new(
                "fake",
                &FakeSourceSettings {
                    blink: Some(255),
                    frame_interval_ms: 10,
                    ..Default::default()
                },
            )),
            processing: Processing {
                style: Style::Custom,
                custom_style: Some(eased),
                ..defaults.processing.clone()
            },
            ..defaults
        };
        let portal = start(blinking, Duration::from_secs(5)).await;
        assert_eq!(portal.target("screen_sync").await.0, StatusCode::NO_CONTENT);
        let response = colors_stream(&portal).await;
        let mut events = response.bytes_stream();
        let mut buffer = String::new();
        next_state(&mut events, &mut buffer, |e| e["type"] == "colors").await;
        let started = std::time::Instant::now();
        let mut sent = 0;
        while started.elapsed() < Duration::from_millis(1500) {
            next_state(&mut events, &mut buffer, |e| e["type"] == "colors").await;
            sent += 1;
        }
        // The cap is an upper bound; a slow machine may send fewer.
        let rate = f64::from(sent) / started.elapsed().as_secs_f64();
        assert!(rate <= 11.0, "{sent} events, {rate:.1}/s");
        drop(events);
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn colors_streams_are_limited_apart_from_previews() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let preview = preview_stream(&portal).await;
        assert_eq!(preview.status(), StatusCode::OK);
        let mut open = Vec::new();
        for _ in 0..8 {
            let response = colors_stream(&portal).await;
            assert_eq!(response.status(), StatusCode::OK);
            open.push(response);
        }
        let refused = colors_stream(&portal).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.json::<Value>().await.unwrap()["kind"], "busy");
        assert_eq!(portal.engine.colors_viewers(), 8);
        assert_eq!(portal.engine.preview_viewers(), 1);

        let foreign = portal
            .request(Method::GET, "/api/v1/colors/stream")
            .header(header::ORIGIN, "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        let wrong = portal
            .request(Method::PUT, "/api/v1/colors/stream")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::METHOD_NOT_ALLOWED);

        let mut streams: Vec<_> = open.into_iter().map(|r| r.bytes_stream()).collect();
        let engine = portal.engine.clone();
        drop(preview);
        portal.shutdown().await.unwrap();
        for events in &mut streams {
            while let Some(chunk) = events.next().await {
                if chunk.is_err() {
                    break;
                }
            }
        }
        drop(streams);
        eventually_async("no colors viewers", LIMIT, || {
            (engine.colors_viewers() == 0).then_some(())
        })
        .await;
    })
    .await;
}
