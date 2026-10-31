//! REST routes and host/origin policy.
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use prysm_api::{ApiFailure, App};
use prysm_contract::{ErrorCode, WebConfig};

async fn guard(State(web): State<WebConfig>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let allowed = !host.is_empty()
        && web
            .allowed_hosts
            .iter()
            .any(|value| value == "*" || value == host);
    let origin_ok = request.headers().get(header::ORIGIN).is_none_or(|origin| {
        origin.to_str().is_ok_and(|origin| {
            origin == format!("http://{host}") || origin == format!("https://{host}")
        })
    });
    if !allowed || !origin_ok {
        return ApiFailure::new(
            StatusCode::FORBIDDEN,
            ErrorCode::InvalidRequest,
            "panel host or request origin is not allowed",
        )
        .into_response();
    }
    let mut response = next.run(request).await;
    for (key, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    ] {
        response.headers_mut().insert(
            header::HeaderName::from_static(key),
            header::HeaderValue::from_static(value),
        );
    }
    response
}

pub(super) fn router(app: App, web: WebConfig) -> Router {
    prysm_api::router()
        .split_for_parts()
        .0
        .fallback(async || {
            (
                StatusCode::NOT_FOUND,
                "404 Not Found\n\nThis is the Prysm REST API. Available endpoints: /api/state and /api/config.\nFor the web UI, open prysm-web (port 8788 by default).\n",
            )
        })
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(web, guard))
        .with_state(app)
}
