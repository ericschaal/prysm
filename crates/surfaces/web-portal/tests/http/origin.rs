//! Which origins may run commands, and the CORS replies they get.
use crate::common::*;
use prysm_runtime::fakes;
use prysm_web_portal::Access;
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn foreign_origins_are_refused_without_running_the_command() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let put = |origin: Option<&str>, host: Option<&str>| {
            let mut request = portal
                .request(Method::PUT, "/api/v1/target")
                .json(&json!({ "target": "processed" }));
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            if let Some(host) = host {
                request = request.header(header::HOST, host);
            }
            request.send()
        };

        for origin in ["http://evil.example", "null", "https://127.0.0.1"] {
            let refused = put(Some(origin), None).await.unwrap();
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{origin}");
            assert!(
                !refused
                    .headers()
                    .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            );
            assert_eq!(
                refused.json::<Value>().await.unwrap(),
                json!({ "kind": "forbidden", "message": "origin denied" })
            );
        }
        let stream = portal
            .request(Method::GET, "/api/v1/state/stream")
            .header(header::ORIGIN, "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(stream.status(), StatusCode::FORBIDDEN);
        let preflight = portal
            .request(Method::OPTIONS, "/api/v1/target")
            .header(header::ORIGIN, "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(preflight.status(), StatusCode::FORBIDDEN);
        assert_eq!(portal.state().await["target"], "off");

        // The portal's own pages, by IP or by name, and clients without Origin.
        let own = format!("http://{}", portal.address);
        let same = put(Some(&own), None).await.unwrap();
        assert_eq!(same.status(), StatusCode::NO_CONTENT);
        assert!(
            !same
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
        let named = put(Some("http://Prysm.lan"), Some("prysm.lan:80"))
            .await
            .unwrap();
        assert_eq!(named.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            put(None, None).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        portal.shutdown().await.unwrap();
    })
    .await;
}

/// The TV app's origin; [`tv_access`] lists it in capitals, as origins
/// compare without case.
const TV: &str = "http://192.168.1.30:3000";

fn tv_access() -> Access {
    Access::Origins(vec!["HTTP://192.168.1.30:3000".into()])
}

#[tokio::test]
async fn a_listed_origin_gets_a_cors_preflight() {
    within_async(LIMIT, async {
        let portal = start_with(fakes::settings(), Duration::from_secs(5), tv_access()).await;
        let preflight = portal
            .request(Method::OPTIONS, "/api/v1/target")
            .header(header::ORIGIN, TV)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
            .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
            .header("access-control-request-private-network", "true")
            .send()
            .await
            .unwrap();
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        let headers = preflight.headers();
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], TV);
        assert_eq!(
            headers[header::ACCESS_CONTROL_ALLOW_METHODS],
            "GET, POST, PUT, DELETE"
        );
        assert_eq!(
            headers[header::ACCESS_CONTROL_ALLOW_HEADERS],
            "content-type"
        );
        assert_eq!(headers["access-control-allow-private-network"], "true");
        assert_eq!(headers[header::VARY], "origin");
        assert_secure(&preflight);
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_listed_origin_gets_cors_on_commands_and_streams() {
    within_async(LIMIT, async {
        let portal = start_with(fakes::settings(), Duration::from_secs(5), tv_access()).await;
        let put = portal
            .request(Method::PUT, "/api/v1/target")
            .header(header::ORIGIN, TV)
            .json(&json!({ "target": "processed" }))
            .send()
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::NO_CONTENT);
        assert_eq!(put.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], TV);
        assert_secure(&put);
        let stream = portal
            .request(Method::GET, "/api/v1/state/stream")
            .header(header::ORIGIN, TV)
            .send()
            .await
            .unwrap();
        assert_eq!(stream.status(), StatusCode::OK);
        assert_eq!(stream.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], TV);
        assert_eq!(stream.headers()[header::CONTENT_TYPE], "text/event-stream");
        assert_secure(&stream);
        drop(stream);
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn an_origin_beside_the_listed_one_is_forbidden() {
    within_async(LIMIT, async {
        let portal = start_with(fakes::settings(), Duration::from_secs(5), tv_access()).await;
        let other = portal
            .request(Method::GET, "/api/v1/state")
            .header(header::ORIGIN, "http://192.168.1.31:3000")
            .send()
            .await
            .unwrap();
        assert_eq!(other.status(), StatusCode::FORBIDDEN);
        assert_secure(&other);
        portal.shutdown().await.unwrap();
    })
    .await;
}

/// A packaged webOS app sends `Origin: null`; listing `null` allows exactly
/// that, not every origin.
#[tokio::test]
async fn listing_null_allows_only_the_null_origin() {
    within_async(LIMIT, async {
        let access = Access::Origins(vec!["null".into()]);
        let portal = start_with(fakes::settings(), Duration::from_secs(5), access).await;
        let put = portal
            .request(Method::PUT, "/api/v1/target")
            .header(header::ORIGIN, "null")
            .json(&json!({ "target": "processed" }))
            .send()
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::NO_CONTENT);
        assert_eq!(put.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "null");
        let preflight = portal
            .request(Method::OPTIONS, "/api/v1/target")
            .header(header::ORIGIN, "null")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
            .send()
            .await
            .unwrap();
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        // The TV app clears a calibration pattern with DELETE.
        let delete = portal
            .request(Method::DELETE, "/api/v1/calibration")
            .header(header::ORIGIN, "null")
            .send()
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            delete.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "null"
        );
        for origin in ["http://anything.example", TV] {
            let refused = portal
                .request(Method::GET, "/api/v1/state")
                .header(header::ORIGIN, origin)
                .send()
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{origin}");
        }
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn any_origin_allows_every_origin() {
    within_async(LIMIT, async {
        let portal = start_with(fakes::settings(), Duration::from_secs(5), Access::AnyOrigin).await;
        for origin in ["null", "http://anything.example"] {
            let state = portal
                .request(Method::GET, "/api/v1/state")
                .header(header::ORIGIN, origin)
                .send()
                .await
                .unwrap();
            assert_eq!(state.status(), StatusCode::OK);
            assert_eq!(state.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
        }
        portal.shutdown().await.unwrap();
    })
    .await;
}
