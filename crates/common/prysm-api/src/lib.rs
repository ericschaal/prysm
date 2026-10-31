//! The real HTTP handlers and the OpenAPI document generated from their router.
#[cfg(feature = "client")]
pub mod client;

use actix::{Message, Recipient};
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use prysm_contract::{
    ApiError, Config, ErrorCode, OperationResponse, StateResponse, Target, UpdateConfig,
};
use tokio::sync::watch;
use utoipa_axum::{router::OpenApiRouter, routes};

#[derive(Clone)]
pub struct App {
    pub set_target: Recipient<SetTarget>,
    pub set_config: Recipient<SetConfig>,
    pub config: watch::Receiver<Config>,
    pub state: watch::Receiver<StateResponse>,
}

#[derive(Message)]
#[rtype(result = "Result<OperationResponse, ApiFailure>")]
pub struct SetTarget(pub Target);

#[derive(Message)]
#[rtype(result = "Result<Config, ApiFailure>")]
pub struct SetConfig(pub UpdateConfig);

#[derive(Debug)]
pub struct ApiFailure(StatusCode, ApiError);
impl ApiFailure {
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self(
            status,
            ApiError {
                code,
                message: message.into(),
            },
        )
    }
    pub fn conflict(message: &str) -> Self {
        Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, message)
    }
    pub fn unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Unavailable,
            "application is shutting down",
        )
    }
}
impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}
fn payload<T>(
    body: std::result::Result<Json<T>, JsonRejection>,
) -> std::result::Result<T, ApiFailure> {
    body.map(|Json(value)| value).map_err(|error| {
        ApiFailure::new(error.status(), ErrorCode::InvalidRequest, error.body_text())
    })
}

#[utoipa::path(get,path="/api/state",operation_id="getState",responses((status=200,body=StateResponse),(status=403,body=ApiError)))]
async fn get_state(State(app): State<App>) -> Json<StateResponse> {
    Json(app.state.borrow().clone())
}
#[utoipa::path(put,path="/api/state",operation_id="setState",request_body=Target,responses((status=202,body=OperationResponse),(status=409,body=ApiError),(status=422,body=ApiError),(status=503,body=ApiError)))]
async fn set_state(
    State(app): State<App>,
    body: std::result::Result<Json<Target>, JsonRejection>,
) -> std::result::Result<(StatusCode, Json<OperationResponse>), ApiFailure> {
    let response = app
        .set_target
        .send(SetTarget(payload(body)?))
        .await
        .map_err(|_| ApiFailure::unavailable())??;
    Ok((StatusCode::ACCEPTED, Json(response)))
}
#[utoipa::path(get,path="/api/config",operation_id="getConfig",responses((status=200,body=Config),(status=403,body=ApiError)))]
async fn get_config(State(app): State<App>) -> Json<Config> {
    Json(app.config.borrow().clone())
}
#[utoipa::path(put,path="/api/config",operation_id="updateConfig",request_body=UpdateConfig,responses((status=200,body=Config),(status=409,body=ApiError),(status=422,body=ApiError),(status=500,body=ApiError),(status=503,body=ApiError)))]
async fn set_config(
    State(app): State<App>,
    body: std::result::Result<Json<UpdateConfig>, JsonRejection>,
) -> std::result::Result<Json<Config>, ApiFailure> {
    Ok(Json(
        app.set_config
            .send(SetConfig(payload(body)?))
            .await
            .map_err(|_| ApiFailure::unavailable())??,
    ))
}
pub fn router() -> OpenApiRouter<App> {
    let mut api = OpenApiRouter::new()
        .routes(routes!(get_state, set_state))
        .routes(routes!(get_config, set_config));
    let info = &mut api.get_openapi_mut().info;
    *info = utoipa::openapi::Info::new("Prysm", env!("CARGO_PKG_VERSION"));
    info.description = Some("Ambient lighting controller".into());
    api
}
