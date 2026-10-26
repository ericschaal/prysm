//! Serve the browser controller and forward its requests to Prysm's REST API.
use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use tower_http::{
    csrf::CsrfLayer, set_header::SetResponseHeaderLayer,
    validate_request::ValidateRequestHeaderLayer,
};

#[derive(Clone)]
struct Backend {
    client: reqwest::Client,
    url: reqwest::Url,
}

/// Build a controller with a fixed backend; configuration remains owned by Prysm.
pub fn router(backend: &str, allowed_hosts: Vec<String>) -> Result<Router> {
    let url = reqwest::Url::parse(backend).context("invalid Prysm API URL")?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "Prysm API URL must be an HTTP base URL"
    );
    anyhow::ensure!(
        !allowed_hosts.is_empty(),
        "configure an allowed controller host"
    );
    let backend = Backend {
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(65))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()?,
        url,
    };
    // Tower requires a full HTTP response as the validation error.
    #[allow(clippy::result_large_err)]
    let host_policy = ValidateRequestHeaderLayer::custom(move |request: &mut Request| {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if host.is_empty()
            || !allowed_hosts
                .iter()
                .any(|allowed| allowed == host || allowed == "*")
            || request
                .uri()
                .authority()
                .is_some_and(|authority| authority.as_str() != host)
        {
            return Err(StatusCode::FORBIDDEN.into_response());
        }
        Ok::<_, Response>(())
    });
    Ok(Router::new()
        .route("/", axum::routing::get(index))
        .route("/assets/{*path}", axum::routing::get(static_asset).layer(
            SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            ),
        ))
        .route("/api/state", axum::routing::get(proxy).put(proxy))
        .route("/api/config", axum::routing::get(proxy).put(proxy))
        .with_state(backend)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(CsrfLayer::new())
        .layer(host_policy)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL, HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"),
        )))
}

async fn proxy(
    State(backend): State<Backend>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        let mut request = backend
            .client
            .request(method, backend.url.join(uri.path())?)
            .body(body);
        if let Some(content_type) = headers.get(header::CONTENT_TYPE) {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        // The backend receives its own Host; browser credentials and Origin stay here.
        let upstream = request.send().await?;
        let mut response = Response::builder().status(upstream.status());
        if let Some(content_type) = upstream.headers().get(header::CONTENT_TYPE) {
            response = response.header(header::CONTENT_TYPE, content_type);
        }
        Ok::<_, anyhow::Error>(response.body(Body::from(upstream.bytes().await?))?)
    }
    .await;
    result.unwrap_or_else(|error| {
        tracing::warn!(%error, "Prysm API unavailable");
        (StatusCode::BAD_GATEWAY, "Prysm API unavailable").into_response()
    })
}

#[derive(rust_embed::Embed)]
#[folder = "control-ui/dist/"]
struct Assets;

fn asset(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => (
            [(
                header::CONTENT_TYPE,
                mime_guess::from_path(path).first_or_octet_stream().as_ref(),
            )],
            file.data.into_owned(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn index() -> Response {
    asset("index.html")
}
async fn static_asset(Path(path): Path<String>) -> Response {
    asset(&format!("assets/{path}"))
}

#[cfg(test)]
mod tests;
