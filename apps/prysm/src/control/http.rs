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
use tower_http::csrf::CsrfLayer;

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
    let authority_ok = request
        .uri()
        .authority()
        .is_none_or(|authority| authority.as_str() == host);
    if !allowed || !origin_ok || !authority_ok {
        return ApiFailure::new(
            StatusCode::FORBIDDEN,
            ErrorCode::InvalidRequest,
            "panel host or request origin is not allowed",
        )
        .into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(axum::http::HeaderValue::from_static("no-store"));
    for (key, value) in [
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

pub(crate) fn router(app: App, web: WebConfig) -> Router {
    prysm_api::router()
        .split_for_parts()
        .0
        .with_state(app)
        .merge(prysm_web::assets_router())
        .fallback(async || {
            (
                StatusCode::NOT_FOUND,
                "404 Not Found\n\nAvailable endpoints: /, /api/state and /api/config.\n",
            )
        })
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(CsrfLayer::new())
        .layer(middleware::from_fn_with_state(web, guard))
}
