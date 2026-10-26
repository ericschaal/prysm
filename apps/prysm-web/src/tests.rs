use super::*;
use axum::{body::to_bytes, extract::Request};
use tower::ServiceExt;

#[tokio::test]
async fn forwards_config_to_the_engine_preserving_status_and_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let backend = Router::new().route(
        "/api/config",
        axum::routing::put(move |request: Request| async move {
            assert_eq!(request.headers()[header::HOST], address.to_string());
            assert!(!request.headers().contains_key(header::ORIGIN));
            assert!(!request.headers().contains_key(header::COOKIE));
            assert!(!request.headers().contains_key(header::AUTHORIZATION));
            assert_eq!(request.headers()[header::CONTENT_TYPE], "application/json");
            let body = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
            assert_eq!(body, r#"{"expected_revision":"2"}"#);
            (
                StatusCode::CONFLICT,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"code":"conflict"}"#,
            )
        }),
    );
    let worker = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
    let controller = router(&format!("http://{address}"), vec!["localhost:8788".into()]).unwrap();
    let response = controller
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/config")
                .header(header::HOST, "localhost:8788")
                .header(header::ORIGIN, "http://localhost:8788")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, "panel-session=private")
                .header(header::AUTHORIZATION, "Bearer panel-token")
                .body(Body::from(r#"{"expected_revision":"2"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        r#"{"code":"conflict"}"#
    );
    worker.abort();
}

#[tokio::test]
async fn rejects_untrusted_requests_and_limits_forwarded_bodies() {
    let controller = router("http://127.0.0.1:1", vec!["localhost:8788".into()]).unwrap();
    for (host, origin, size, expected) in [
        (
            "attacker.test",
            "http://attacker.test",
            0,
            StatusCode::FORBIDDEN,
        ),
        (
            "localhost:8788",
            "http://attacker.test",
            0,
            StatusCode::FORBIDDEN,
        ),
        (
            "localhost:8788",
            "http://localhost:8788",
            65537,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            "localhost:8788",
            "http://localhost:8788",
            0,
            StatusCode::BAD_GATEWAY,
        ),
    ] {
        let response = controller
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header(header::HOST, host)
                    .header(header::ORIGIN, origin)
                    .body(Body::from(vec![0; size]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}

#[test]
fn rejects_non_base_backend_urls() {
    for url in [
        "file:///tmp/prysm",
        "http://user@prysm/",
        "http://prysm/api",
        "http://prysm/?q=1",
        "http://prysm/#api",
    ] {
        assert!(router(url, vec!["localhost:8788".into()]).is_err());
    }
}

#[tokio::test]
async fn serves_the_controller_without_a_running_engine() {
    let controller = router("http://127.0.0.1:1", vec!["localhost:8788".into()]).unwrap();
    let response = controller
        .oneshot(
            Request::builder()
                .uri("/")
                .header(header::HOST, "localhost:8788")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/html");
    let html = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let html = std::str::from_utf8(&html).unwrap();
    assert!(html.contains("<div id=\"root\">"));
    let script = html
        .split("src=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let response = router("http://127.0.0.1:1", vec!["localhost:8788".into()])
        .unwrap()
        .oneshot(
            Request::builder()
                .uri(script)
                .header(header::HOST, "localhost:8788")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert!(
        !to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn enforces_host_and_standard_csrf_policy() {
    let controller = router("http://127.0.0.1:1", vec!["localhost:8788".into()]).unwrap();
    for (method, uri, host, origin, site, expected) in [
        ("GET", "/", "", None, None, StatusCode::FORBIDDEN),
        (
            "GET",
            "/",
            "attacker.test",
            None,
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            "GET",
            "/",
            "localhost:8788",
            Some("http://attacker.test"),
            Some("cross-site"),
            StatusCode::OK,
        ),
        (
            "PUT",
            "/api/config",
            "localhost:8788",
            None,
            None,
            StatusCode::BAD_GATEWAY,
        ),
        (
            "PUT",
            "/api/config",
            "localhost:8788",
            None,
            Some("same-origin"),
            StatusCode::BAD_GATEWAY,
        ),
        (
            "PUT",
            "/api/config",
            "localhost:8788",
            None,
            Some("cross-site"),
            StatusCode::FORBIDDEN,
        ),
        (
            "PUT",
            "/api/config",
            "localhost:8788",
            None,
            Some("same-site"),
            StatusCode::FORBIDDEN,
        ),
        (
            "PUT",
            "/api/config",
            "localhost:8788",
            Some("null"),
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            "PUT",
            "http://attacker.test/api/config",
            "localhost:8788",
            Some("http://attacker.test"),
            None,
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, host);
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        if let Some(site) = site {
            request = request.header("sec-fetch-site", site);
        }
        let response = controller
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            expected,
            "{method} {uri}: host={host}, origin={origin:?}, site={site:?}"
        );
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            response.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
    }
}

#[tokio::test]
async fn wildcard_hosts_still_require_a_host_and_csrf_validation() {
    let controller = router("http://127.0.0.1:1", vec!["*".into()]).unwrap();
    for (host, expected) in [("", StatusCode::FORBIDDEN), ("panel.local", StatusCode::OK)] {
        let response = controller
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let response = controller
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/state")
                .header(header::HOST, "panel.local")
                .header(header::ORIGIN, "http://attacker.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
