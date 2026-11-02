use super::*;
use axum::{
    body::{Body, to_bytes},
    extract::Request,
};
use tower::ServiceExt;

#[tokio::test]
async fn serves_embedded_html_and_immutable_assets() {
    let controller = assets_router();
    let response = controller
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
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
    let response = assets_router()
        .oneshot(Request::builder().uri(script).body(Body::empty()).unwrap())
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
