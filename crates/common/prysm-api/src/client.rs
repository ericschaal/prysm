//! Typed REST requests using the same contract as the HTTP handlers.
use prysm_contract::{ApiError, ErrorCode, OperationResponse, StateResponse, Target};
use serde::de::DeserializeOwned;

async fn json<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T, ApiError> {
    let response = request.send().await.map_err(|error| ApiError {
        code: ErrorCode::Unavailable,
        message: format!("Cannot reach controller: {error}"),
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(response
            .json::<ApiError>()
            .await
            .unwrap_or_else(|_| ApiError {
                code: ErrorCode::Backend,
                message: format!("Controller returned HTTP {status}"),
            }));
    }
    response.json().await.map_err(|error| ApiError {
        code: ErrorCode::Backend,
        message: format!("Invalid controller response: {error}"),
    })
}

/// Read controller state using the supplied HTTP client's transport settings.
pub async fn get_state(http: &reqwest::Client, base_url: &str) -> Result<StateResponse, ApiError> {
    json(http.get(format!("{}/api/state", base_url.trim_end_matches('/')))).await
}

/// Request a power/source change and return its operation ID.
pub async fn set_target(
    http: &reqwest::Client,
    base_url: &str,
    target: Target,
) -> Result<OperationResponse, ApiError> {
    json(
        http.put(format!("{}/api/state", base_url.trim_end_matches('/')))
            .json(&target),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn reports_invalid_json_http_errors_api_errors_and_disconnects() {
        let count = Arc::new(AtomicUsize::new(0));
        let router = Router::new().route(
            "/api/state",
            get(move || {
                let count = count.clone();
                async move {
                    match count.fetch_add(1, Ordering::Relaxed) {
                        0 => (StatusCode::OK, "not JSON").into_response(),
                        1 => (StatusCode::BAD_GATEWAY, "not an API error").into_response(),
                        _ => (
                            StatusCode::CONFLICT,
                            Json(ApiError {
                                code: ErrorCode::Conflict,
                                message: "a change is already running".into(),
                            }),
                        )
                            .into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let (shutdown, stop) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stop.await;
                })
                .await
                .unwrap();
        });
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(1))
            .build()
            .unwrap();
        let error = get_state(&http, &base).await.unwrap_err();
        assert!(matches!(error.code, ErrorCode::Backend));
        assert!(error.message.starts_with("Invalid controller response"));
        let error = get_state(&http, &base).await.unwrap_err();
        assert!(error.message.contains("502"));
        let error = get_state(&http, &base).await.unwrap_err();
        assert!(matches!(error.code, ErrorCode::Conflict));
        assert_eq!(error.message, "a change is already running");
        shutdown.send(()).unwrap();
        server.await.unwrap();
        let error = get_state(&http, &base).await.unwrap_err();
        assert!(matches!(error.code, ErrorCode::Unavailable));
    }
}
