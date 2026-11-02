//! Embedded production web UI assets; the host applies HTTP access policy.
use axum::{
    Router,
    extract::Path,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use tower_http::set_header::SetResponseHeaderLayer;

/// Embedded UI routes, protected by the hosting application's HTTP policy.
pub fn assets_router() -> Router {
    Router::new().route("/", axum::routing::get(index)).route(
        "/assets/{*path}",
        axum::routing::get(static_asset).layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        )),
    )
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
