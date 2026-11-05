//! Explicit wire roots. Private calibration replies are separate from public snapshots.
use crate::{
    application::LightingTarget, capture::CaptureCapabilities, output::OutputCapabilities,
};
use crate::{
    application::{Accepted, OperationStatus},
    client::{
        CalibrationPresentation, ClientFailure, ClientSnapshot, ClientUpdate, ConfigureRequest,
        EndPresentationRequest, PatternRequest, PlaybackFeedback, PresentationFeedback,
        TargetRequest,
    },
    identity::{InstanceId, OperationId, SettingsRevision},
    observation::Cursor,
};
use serde_json::Value;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(components(schemas(LightingTarget, CaptureCapabilities, OutputCapabilities, ClientSnapshot<Value, Value>, ClientUpdate<Value, Value>, ConfigureRequest<Value>, TargetRequest, ClientFailure, Accepted, OperationStatus, OperationId, SettingsRevision, InstanceId, Cursor, CalibrationPresentation, PatternRequest, PlaybackFeedback, PresentationFeedback, EndPresentationRequest)))]
struct Contract;

pub fn document() -> utoipa::openapi::OpenApi {
    Contract::openapi()
}

pub fn resolution() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
    use utoipa::openapi::schema::{ArrayBuilder, ObjectBuilder, Type};
    ArrayBuilder::new()
        .prefix_items([3840, 2160].map(|maximum| {
            ObjectBuilder::new()
                .schema_type(Type::Integer)
                .minimum(Some(1.0))
                .maximum(Some(f64::from(maximum)))
                .build()
        }))
        .min_items(Some(2))
        .max_items(Some(2))
        .into()
}
