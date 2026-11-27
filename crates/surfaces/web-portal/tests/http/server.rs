//! Construction, assets, headers and the server's own lifecycle.
use crate::common::*;
use prysm_ports::{AdapterSettings, Settings};
use prysm_runtime::fakes::{self, FakeSourceSettings};
use prysm_web_portal::{Access, Caching, FrontendAsset, WebPortal};
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn construction_rejects_bad_assets_and_origins() {
    let field = |result: prysm_ports::Result<WebPortal>| match result {
        Err(prysm_ports::Error::Invalid(invalid)) => invalid.field,
        other => panic!("expected a validation error, got {other:?}"),
    };
    let portal = |assets, access| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        WebPortal::from_listener(listener, assets, access)
    };
    portal(ASSETS, Access::SameOrigin).unwrap();
    portal(ASSETS, Access::AnyOrigin).unwrap();
    let origins = |origins: &[&str]| Access::Origins(origins.iter().map(|o| (*o).into()).collect());
    portal(
        ASSETS,
        origins(&[
            "http://192.168.1.20:3000",
            "HTTPS://TV.example:443",
            "app://x",
            "null",
        ]),
    )
    .unwrap();
    for origin in [
        "NULL",
        " null",
        "*",
        "192.168.1.20",
        "http://tv.example/",
        "http://tv.example/path",
        "http://user@tv.example",
        "http://tv.example?x",
        "",
    ] {
        assert_eq!(
            field(portal(ASSETS, origins(&[origin]))),
            "portal.access",
            "{origin}"
        );
    }
    for path in ["/api/v1/state", "relative", "/../x", "/a%2e", "/index.html"] {
        let assets: &'static [FrontendAsset] = Box::leak(Box::new([
            ASSETS[0].clone(),
            FrontendAsset {
                path: Box::leak(path.to_owned().into_boxed_str()),
                ..ASSETS[1].clone()
            },
        ]));
        assert_eq!(field(portal(assets, Access::SameOrigin)), "portal.assets");
    }
    let assets: &'static [FrontendAsset] = Box::leak(Box::new([FrontendAsset {
        caching: Caching::Revalidate,
        ..ASSETS[0].clone()
    }]));
    assert_eq!(field(portal(assets, Access::SameOrigin)), "portal.assets");
}

#[tokio::test]
async fn asset_serving_and_unauthenticated_api() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let url = |path: &str| format!("http://{}{path}", portal.address);
        let client = &portal.client;

        let page = client.get(url("/")).send().await.unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(
            page.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_secure(&page);
        assert!(!page.headers().contains_key(header::VARY));
        let page = page.text().await.unwrap();
        let start =
            r#"<head><title>Prysm</title><script type="application/json" id="prysm-state">"#;
        let block = page.strip_prefix("<!doctype html><html>").unwrap();
        let block = block.strip_prefix(start).unwrap();
        let (state, rest) = block.split_once("</script>").unwrap();
        assert_eq!(rest, "</head><body></body></html>");
        assert_eq!(json_body("/", state), portal.state().await);
        let named = client.get(url("/index.html")).send().await.unwrap();
        assert_secure(&named);
        assert_eq!(named.text().await.unwrap(), page);
        let kept = "public, max-age=31536000, immutable";
        let script = client.get(url("/assets/app.js")).send().await.unwrap();
        assert_eq!(script.headers()[header::CONTENT_TYPE], "text/javascript");
        assert_secure_kept(&script, kept);
        assert_eq!(script.headers()[header::VARY], "accept-encoding");
        assert!(!script.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(script.text().await.unwrap(), "console.log('prysm')");
        for accepted in ["gzip", "br;q=1.0, GZip;q=0.5", "*"] {
            let script = client
                .get(url("/assets/app.js"))
                .header(header::ACCEPT_ENCODING, accepted)
                .send()
                .await
                .unwrap();
            assert_eq!(script.headers()[header::CONTENT_ENCODING], "gzip");
            assert_secure_kept(&script, kept);
            assert_eq!(script.bytes().await.unwrap().as_ref(), b"compressed");
        }
        for refused in ["br", "gzip;q=0", "identity", "gzip;q=0, *"] {
            let script = client
                .get(url("/assets/app.js"))
                .header(header::ACCEPT_ENCODING, refused)
                .send()
                .await
                .unwrap();
            assert!(!script.headers().contains_key(header::CONTENT_ENCODING));
            assert_eq!(script.text().await.unwrap(), "console.log('prysm')");
        }
        let head = client.head(url("/assets/app.js")).send().await.unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "20");
        assert_secure_kept(&head, kept);
        assert!(head.bytes().await.unwrap().is_empty());
        let gone = client.head(url("/assets/old.js")).send().await.unwrap();
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
        let missing = client.get(url("/missing")).send().await.unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_secure(&missing);
        let named = client
            .get(url("/api/v1/state"))
            .header(header::HOST, "prysm.lan:8787")
            .send()
            .await
            .unwrap();
        assert_eq!(named.status(), StatusCode::OK);
        assert_eq!(named.headers()[header::VARY], "origin");
        assert_secure(&named);

        for path in [
            "/api/v1/state",
            "/api/v1/state/stream",
            "/api/v1/preview/stream",
            "/api/v1/colors/stream",
            "/api/v1/target",
            "/api/v1/calibration",
            "/api/v1/processing/trial",
            "/api/v1/settings",
            "/api/v1/capture/modes",
            "/api/v1/candidates/capture/fake",
        ] {
            let reply = portal.request(Method::PATCH, path).send().await.unwrap();
            assert_eq!(reply.status(), StatusCode::METHOD_NOT_ALLOWED, "{path}");
            assert_eq!(
                reply.json::<Value>().await.unwrap()["kind"],
                "method_not_allowed"
            );
        }
        for path in [
            "/api/v1/nothing",
            "/metrics",
            "/api/v1/capture/stream",
            "/api/v1/candidates/output/",
            "/api/v1/candidates/output/fake/more",
            "/api/v1/candidates/processing/fake",
        ] {
            for method in [Method::GET, Method::PATCH] {
                let reply = portal.request(method, path).send().await.unwrap();
                assert_eq!(reply.status(), StatusCode::NOT_FOUND, "{path}");
            }
        }
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn faulted_engine_is_unavailable() {
    within_async(LIMIT, async {
        let slow_close = Settings {
            capture: Some(AdapterSettings::new(
                "fake",
                &FakeSourceSettings {
                    close_delay_ms: 500,
                    ..FakeSourceSettings::default()
                },
            )),
            ..fakes::settings()
        };
        let portal = start(slow_close, Duration::from_millis(50)).await;
        assert_eq!(portal.target("screen_sync").await.0, StatusCode::NO_CONTENT);
        portal
            .engine
            .snapshot()
            .wait_for(|snapshot| matches!(snapshot.session, prysm_ports::SessionState::ScreenSync))
            .await
            .unwrap();
        assert_eq!(portal.target("off").await.0, StatusCode::NO_CONTENT);
        portal
            .engine
            .snapshot()
            .wait_for(|snapshot| matches!(snapshot.session, prysm_ports::SessionState::Faulted(_)))
            .await
            .unwrap();
        let (status, body) = portal.target("screen_sync").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["kind"], "faulted");
        let state = portal.state().await;
        assert_eq!(
            state["session"],
            json!({ "state": "faulted", "message": "The video source did not close in time." })
        );
        assert!(portal.shutdown().await.is_err());
    })
    .await;
}

/// A reply sent before the request body was read (here a 403) must reach the
/// client: closing with unread bytes would reset the connection and discard it.
#[tokio::test]
async fn early_replies_survive_an_unread_request_body() {
    use std::io::{Read, Write};
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let address = portal.address;
        let reply = tokio::task::spawn_blocking(move || {
            let body = vec![b' '; 256 * 1024];
            let mut socket = std::net::TcpStream::connect(address).unwrap();
            socket.set_read_timeout(Some(LIMIT)).unwrap();
            let mut writer = socket.try_clone().unwrap();
            let sender = std::thread::spawn(move || {
                let head = format!(
                    "PUT /api/v1/target HTTP/1.1\r\nHost: {address}\r\n\
                     Origin: http://evil.example\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                // The portal may stop reading; the reply is what matters.
                let _ = writer.write_all(head.as_bytes());
                let _ = writer.write_all(&body);
            });
            // Let the reply and any reset arrive before reading.
            std::thread::sleep(Duration::from_millis(200));
            let mut reply = Vec::new();
            let read = socket.read_to_end(&mut reply);
            sender.join().unwrap();
            read.map(|_| String::from_utf8_lossy(&reply).into_owned())
        })
        .await
        .unwrap()
        .expect("the portal reset the connection");
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert!(reply.contains(r#""kind":"forbidden""#), "{reply}");
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn shutdown_closes_the_listener_promptly() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let address = portal.address;
        assert_eq!(portal.state().await["target"], "off");
        let started = std::time::Instant::now();
        portal.shutdown().await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    })
    .await;
}
