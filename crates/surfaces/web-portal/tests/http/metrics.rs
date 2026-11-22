//! The metrics endpoint.
use crate::common::*;
use prysm_runtime::fakes;
use prysm_web_portal::Access;
use reqwest::{Method, StatusCode, header};
use std::time::Duration;

#[tokio::test]
async fn metrics_are_off_by_default() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(2)).await;
        for method in [Method::GET, Method::POST] {
            let response = portal.request(method, "/metrics").send().await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn enabled_metrics_count_frames_and_follow_the_origin_policy() {
    within_async(LIMIT, async {
        let portal = start_full(
            fakes::settings(),
            Duration::from_secs(2),
            Access::SameOrigin,
            true,
        )
        .await;
        assert_eq!(portal.target("processed").await.0, StatusCode::NO_CONTENT);
        assert!(portal.log.wait_for("output.write:", 3).await);

        let response = portal
            .request(Method::GET, "/metrics")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; version=0.0.4; charset=utf-8"
        );
        assert_secure(&response);
        let text = response.text().await.unwrap();
        assert!(text.contains("# TYPE prysm_frames_received_total counter\n"));
        assert!(text.contains("prysm_target{target=\"processed\"} 1\n"));
        assert!(text.contains("prysm_session_state{state=\"running\"} 1\n"));
        assert!(text.contains("prysm_source_opens_total 1\n"));
        let received = text
            .lines()
            .find_map(|line| line.strip_prefix("prysm_frames_received_total "))
            .and_then(|count| count.parse::<u64>().ok())
            .unwrap();
        assert!(received >= 3, "{text}");

        for method in [Method::POST, Method::PUT, Method::DELETE] {
            let response = portal.request(method, "/metrics").send().await.unwrap();
            assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        }
        let foreign = portal
            .request(Method::GET, "/metrics")
            .header(header::ORIGIN, "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        portal.shutdown().await.unwrap();
    })
    .await;
}
