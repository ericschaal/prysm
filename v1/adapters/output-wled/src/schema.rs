//! Public wire surface owned by this adapter. Add roots here as its API grows.
use super::WledSettings;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(components(schemas(WledSettings)))]
struct Contract;

pub fn document() -> utoipa::openapi::OpenApi {
    Contract::openapi()
}
