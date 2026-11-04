//! REST routes and host/origin policy.
use crate::config::WebConfig;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use prysm_api::{ApiFailure, App};
use prysm_contract::ErrorCode;
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
    let relay = request.uri().path() == "/calibration-tv.html";
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(axum::http::HeaderValue::from_static("no-store"));
    for (key, value) in [
        ("x-content-type-options", "nosniff"),
        (
            "content-security-policy",
            if relay {
                "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors file:; base-uri 'none'; form-action 'self'"
            } else {
                "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"
            },
        ),
    ] {
        response.headers_mut().insert(
            header::HeaderName::from_static(key),
            header::HeaderValue::from_static(value),
        );
    }
    response
}

pub(crate) fn router(app: App, web: WebConfig, preview: Router) -> Router {
    protect(
        prysm_api::router().split_for_parts().0.with_state(app),
        web,
        preview,
    )
}

fn protect(router: Router, web: WebConfig, preview: Router) -> Router {
    router
        .merge(prysm_web::assets_router())
        .merge(preview)
        .fallback(async || {
            (
                StatusCode::NOT_FOUND,
                "404 Not Found\n\nAvailable endpoints: /, /api/state and /api/settings.\n",
            )
        })
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(CsrfLayer::new())
        .layer(middleware::from_fn_with_state(web, guard))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt as _;

    #[tokio::test]
    async fn calibration_relay_does_not_relax_api_host_origin_or_csrf_policy() {
        let router = || {
            protect(
                Router::new().route(
                    "/api/calibration",
                    axum::routing::post(|| async { StatusCode::OK }),
                ),
                WebConfig {
                    listen: "127.0.0.1:8787".parse().unwrap(),
                    allowed_hosts: vec!["localhost:8787".into()],
                },
                Router::new(),
            )
        };
        for (host, origin, site, expected) in [
            (
                "localhost:8787",
                "http://localhost:8787",
                "same-origin",
                StatusCode::OK,
            ),
            (
                "localhost:8787",
                "null",
                "cross-site",
                StatusCode::FORBIDDEN,
            ),
            (
                "localhost:8787",
                "http://evil.invalid",
                "cross-site",
                StatusCode::FORBIDDEN,
            ),
            (
                "evil.invalid",
                "http://evil.invalid",
                "same-origin",
                StatusCode::FORBIDDEN,
            ),
        ] {
            let response = router()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/calibration")
                        .header(header::HOST, host)
                        .header(header::ORIGIN, origin)
                        .header("sec-fetch-site", site)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        for (path, policy) in [
            ("/", "frame-ancestors 'none'"),
            ("/calibration-tv.html", "frame-ancestors file:"),
        ] {
            let response = router()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::HOST, "localhost:8787")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                response.headers()["content-security-policy"]
                    .to_str()
                    .unwrap()
                    .contains(policy)
            );
        }
    }
}
