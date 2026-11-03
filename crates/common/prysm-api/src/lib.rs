//! HTTP transport for the application's settings and lighting commands.
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use prysm_contract::{
    ApiError, ErrorCode, OperationResponse, SettingsView, StateResponse, Target, UpdateSettings,
};
use tokio::sync::{mpsc, oneshot, watch};
use utoipa_axum::{router::OpenApiRouter, routes};

/// A cloneable application handle; the controller owns all mutation ordering.
#[derive(Clone)]
pub struct App {
    commands: mpsc::Sender<Command>,
    settings: watch::Receiver<SettingsView>,
    state: watch::Receiver<Option<StateResponse>>,
}

impl App {
    #[must_use]
    pub fn new(
        commands: mpsc::Sender<Command>,
        settings: watch::Receiver<SettingsView>,
        state: watch::Receiver<Option<StateResponse>>,
    ) -> Self {
        Self {
            commands,
            settings,
            state,
        }
    }

    #[must_use]
    pub fn settings(&self) -> SettingsView {
        self.settings.borrow().clone()
    }

    pub fn state(&self) -> Result<StateResponse, CommandError> {
        self.state.borrow().clone().ok_or(CommandError::Conflict(
            "complete settings before controlling the lights",
        ))
    }

    pub async fn update_settings(
        &self,
        update: UpdateSettings,
    ) -> Result<SettingsView, CommandError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::UpdateSettings(Box::new(update), reply))
            .await
            .map_err(|_| CommandError::Unavailable)?;
        response.await.map_err(|_| CommandError::Unavailable)?
    }

    pub async fn set_target(&self, target: Target) -> Result<OperationResponse, CommandError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::SetTarget(target, reply))
            .await
            .map_err(|_| CommandError::Unavailable)?;
        response.await.map_err(|_| CommandError::Unavailable)?
    }
}

/// Accepted commands continue even when their reply receiver is dropped.
#[derive(Debug)]
pub enum Command {
    UpdateSettings(
        Box<UpdateSettings>,
        oneshot::Sender<Result<SettingsView, CommandError>>,
    ),
    SetTarget(
        Target,
        oneshot::Sender<Result<OperationResponse, CommandError>>,
    ),
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    Conflict(&'static str),
    #[error("invalid settings: {0:#}")]
    Invalid(#[source] anyhow::Error),
    #[error("could not save settings: {0:#}")]
    Persistence(#[source] anyhow::Error),
    #[error("lighting operation failed: {0:#}")]
    Backend(#[source] anyhow::Error),
    #[error("application is shutting down")]
    Unavailable,
}

impl CommandError {
    #[must_use]
    pub fn report(&self) -> ApiError {
        let code = match self {
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::Invalid(_) => ErrorCode::InvalidRequest,
            Self::Persistence(_) => ErrorCode::Persistence,
            Self::Backend(_) => ErrorCode::Backend,
            Self::Unavailable => ErrorCode::Unavailable,
        };
        ApiError {
            code,
            message: self.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct ApiFailure(StatusCode, ApiError);

impl ApiFailure {
    #[must_use]
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self(
            status,
            ApiError {
                code,
                message: message.into(),
            },
        )
    }
}

impl From<CommandError> for ApiFailure {
    fn from(error: CommandError) -> Self {
        let status = match error {
            CommandError::Conflict(_) => StatusCode::CONFLICT,
            CommandError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            CommandError::Persistence(_) => StatusCode::INTERNAL_SERVER_ERROR,
            CommandError::Backend(_) => StatusCode::BAD_GATEWAY,
            CommandError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        Self(status, error.report())
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

fn payload<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiFailure> {
    body.map(|Json(value)| value).map_err(|error| {
        ApiFailure::new(error.status(), ErrorCode::InvalidRequest, error.body_text())
    })
}

#[utoipa::path(get,path="/api/settings",operation_id="getSettings",responses((status=200,body=SettingsView),(status=403,body=ApiError)))]
async fn get_settings(State(app): State<App>) -> Json<SettingsView> {
    Json(app.settings())
}

#[utoipa::path(put,path="/api/settings",operation_id="updateSettings",request_body=UpdateSettings,responses((status=200,body=SettingsView),(status=409,body=ApiError),(status=422,body=ApiError),(status=500,body=ApiError),(status=502,body=ApiError),(status=503,body=ApiError)))]
async fn update_settings(
    State(app): State<App>,
    body: Result<Json<UpdateSettings>, JsonRejection>,
) -> Result<Json<SettingsView>, ApiFailure> {
    Ok(Json(app.update_settings(payload(body)?).await?))
}

#[utoipa::path(get,path="/api/state",operation_id="getState",responses((status=200,body=StateResponse),(status=409,body=ApiError),(status=403,body=ApiError)))]
async fn get_state(State(app): State<App>) -> Result<Json<StateResponse>, ApiFailure> {
    Ok(Json(app.state()?))
}

#[utoipa::path(put,path="/api/state",operation_id="setState",request_body=Target,responses((status=202,body=OperationResponse),(status=409,body=ApiError),(status=422,body=ApiError),(status=502,body=ApiError),(status=503,body=ApiError)))]
async fn set_state(
    State(app): State<App>,
    body: Result<Json<Target>, JsonRejection>,
) -> Result<(StatusCode, Json<OperationResponse>), ApiFailure> {
    Ok((
        StatusCode::ACCEPTED,
        Json(app.set_target(payload(body)?).await?),
    ))
}

#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(prysm_contract::PreviewMessage)))]
struct PreviewSchema;

pub fn router() -> OpenApiRouter<App> {
    let mut api = OpenApiRouter::new()
        .routes(routes!(get_settings, update_settings))
        .routes(routes!(get_state, set_state));
    api.get_openapi_mut()
        .merge(<PreviewSchema as utoipa::OpenApi>::openapi());
    let info = &mut api.get_openapi_mut().info;
    *info = utoipa::openapi::Info::new("Prysm", env!("CARGO_PKG_VERSION"));
    info.description = Some("Ambient lighting controller".into());
    api
}
