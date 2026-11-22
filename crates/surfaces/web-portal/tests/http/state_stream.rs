//! The state event stream.
use crate::common::*;
use futures::StreamExt;
use prysm_runtime::fakes;
use reqwest::{Method, StatusCode, header};
use serde_json::Value;
use std::time::Duration;

#[tokio::test]
async fn state_stream_delivers_changes_and_ends_on_shutdown() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let response = portal
            .request(Method::GET, "/api/v1/state/stream")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let mut events = response.bytes_stream();
        let mut buffer = String::new();
        let first = next_state(&mut events, &mut buffer, |_| true).await;
        assert_eq!(first["session"]["state"], "stopped");

        let (status, _) = portal.target("screen_sync").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let running = next_state(&mut events, &mut buffer, |s| {
            s["session"]["state"] == "screen_sync"
        })
        .await;
        assert_eq!(running["target"], "screen_sync");

        portal.shutdown().await.unwrap();
        // The stream ends instead of hanging once the portal stops.
        while let Some(chunk) = events.next().await {
            if chunk.is_err() {
                break;
            }
        }
    })
    .await;
}

#[tokio::test]
async fn state_streams_are_limited() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let mut open = Vec::new();
        for _ in 0..8 {
            let response = portal
                .request(Method::GET, "/api/v1/state/stream")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            open.push(response);
        }
        let refused = portal
            .request(Method::GET, "/api/v1/state/stream")
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.json::<Value>().await.unwrap()["kind"], "busy");
        drop(open);
        portal.shutdown().await.unwrap();
    })
    .await;
}
