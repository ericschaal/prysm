//! The preview stream: frames while running, its cadence, its viewers and origins.
use crate::common::*;
use futures::StreamExt;
use prysm_runtime::fakes;
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn preview_is_idle_while_off_and_streams_frames_while_running() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let (status, _) = portal.target("processed").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // Lighting runs, nobody watches: the session publishes nothing.
        assert_eq!(portal.engine.preview_viewers(), 0);
        assert_eq!(portal.target("off").await.0, StatusCode::NO_CONTENT);

        let response = preview_stream(&portal).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        assert_secure(&response);
        assert_eq!(portal.engine.preview_viewers(), 1);
        let mut events = response.bytes_stream();
        let mut buffer = String::new();
        let first = next_state(&mut events, &mut buffer, |_| true).await;
        assert_eq!(first, json!({ "type": "idle" }));

        assert_eq!(portal.target("processed").await.0, StatusCode::NO_CONTENT);
        let frame = next_state(&mut events, &mut buffer, |e| e["type"] != "idle").await;
        assert_eq!(frame["type"], "frame");
        assert_eq!(
            (frame["width"].clone(), frame["height"].clone()),
            (json!(16), json!(9))
        );
        assert_eq!(frame["source_width"], 16);
        assert_eq!(
            frame["viewport"],
            json!({ "x": 0, "y": 0, "width": 16, "height": 9 })
        );
        for edge in ["top", "right", "bottom", "left"] {
            let colors = frame["colors"][edge].as_array().unwrap();
            assert!(!colors.is_empty() && colors.len() <= 256, "{edge}");
            assert_eq!(colors[0].as_array().unwrap().len(), 3);
        }
        assert!(
            frame["jpeg"].as_str().unwrap().starts_with("/9j/"),
            "JPEG SOI"
        );
        let later = next_state(&mut events, &mut buffer, |e| e["type"] == "frame").await;
        assert!(later["sequence"].as_u64() > frame["sequence"].as_u64());

        // The session ends mid-stream: the stream stays open and says so.
        assert_eq!(portal.target("off").await.0, StatusCode::NO_CONTENT);
        next_state(&mut events, &mut buffer, |e| e["type"] == "idle").await;

        drop(events);
        viewers(&portal, 0).await;
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn preview_cadence_is_capped_at_eight_per_second() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        assert_eq!(portal.target("processed").await.0, StatusCode::NO_CONTENT);
        let response = preview_stream(&portal).await;
        let mut events = response.bytes_stream();
        let mut buffer = String::new();
        next_state(&mut events, &mut buffer, |e| e["type"] == "frame").await;
        // The fake source produces about 500 frames per second.
        let started = std::time::Instant::now();
        let mut frames = 0;
        while started.elapsed() < Duration::from_millis(1500) {
            next_state(&mut events, &mut buffer, |e| e["type"] == "frame").await;
            frames += 1;
        }
        // The cap is an upper bound; a slow machine may send fewer.
        let rate = f64::from(frames) / started.elapsed().as_secs_f64();
        assert!(frames >= 2 && rate <= 9.0, "{frames} frames, {rate:.1}/s");
        drop(events);
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn preview_viewers_are_limited_and_streams_end_on_shutdown() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let mut open = Vec::new();
        for _ in 0..4 {
            let response = preview_stream(&portal).await;
            assert_eq!(response.status(), StatusCode::OK);
            open.push(response);
        }
        let refused = preview_stream(&portal).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.json::<Value>().await.unwrap()["kind"], "busy");
        assert_eq!(portal.engine.preview_viewers(), 4);
        // Preview viewers do not use up state streams.
        let state = portal
            .request(Method::GET, "/api/v1/state/stream")
            .send()
            .await
            .unwrap();
        assert_eq!(state.status(), StatusCode::OK);
        drop(state);

        let mut streams: Vec<_> = open.into_iter().map(|r| r.bytes_stream()).collect();
        let engine = portal.engine.clone();
        portal.shutdown().await.unwrap();
        for events in &mut streams {
            while let Some(chunk) = events.next().await {
                if chunk.is_err() {
                    break;
                }
            }
        }
        drop(streams);
        eventually_async("no preview viewers", LIMIT, || {
            (engine.preview_viewers() == 0).then_some(())
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn preview_follows_the_origin_policy() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let foreign = portal
            .request(Method::GET, "/api/v1/preview/stream")
            .header(header::ORIGIN, "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        assert_eq!(portal.engine.preview_viewers(), 0);

        let own = portal
            .request(Method::GET, "/api/v1/preview/stream")
            .header(header::ORIGIN, format!("http://{}", portal.address))
            .send()
            .await
            .unwrap();
        assert_eq!(own.status(), StatusCode::OK);
        assert!(
            own.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
        drop(own);

        let wrong = portal
            .request(Method::PUT, "/api/v1/preview/stream")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::METHOD_NOT_ALLOWED);
        portal.shutdown().await.unwrap();
    })
    .await;
}
