//! Build-time contract composition, parallel to the runtime host assemblies.
//! Documents contain reviewed public schemas; HTTP paths are not defined yet.
use std::collections::BTreeMap;
use utoipa::openapi::OpenApi;

pub fn server() -> Result<OpenApi, String> {
    compose(
        "Prysm server",
        [
            prysm_contract::schema::document(),
            prysm_capture_desktop::schema::document(),
            prysm_remote_capture::schema::document(),
            crate::server::document(),
            prysm_output_wled::schema::document(),
        ],
    )
}

pub fn webos() -> Result<OpenApi, String> {
    compose(
        "Prysm webOS",
        [
            prysm_contract::schema::document(),
            prysm_capture_webos::schema::document(),
            prysm_output_wled::schema::document(),
        ],
    )
}

/// Package subpaths are selected here; no individual Rust type names enter codegen.
pub fn documents() -> Result<BTreeMap<&'static str, OpenApi>, String> {
    Ok(BTreeMap::from([
        ("application", prysm_contract::schema::document()),
        ("tv-host", crate::tv::document()),
        ("desktop", prysm_capture_desktop::schema::document()),
        ("remote-capture", prysm_remote_capture::schema::document()),
        ("webos", prysm_capture_webos::schema::document()),
        ("wled", prysm_output_wled::schema::document()),
        ("host-server", server()?),
        ("host-webos", webos()?),
    ]))
}

/// Compose schema-only library documents without silently shadowing a public type.
pub fn compose(
    title: &str,
    documents: impl IntoIterator<Item = OpenApi>,
) -> Result<OpenApi, String> {
    let mut result = OpenApi::default();
    result.info.title = title.into();
    result.info.version = env!("CARGO_PKG_VERSION").into();
    for document in documents {
        // Route composition belongs to the future HTTP adapter, not this schema manifest.
        if !document.paths.paths.is_empty() {
            return Err("schema composition does not accept HTTP routes".into());
        }
        if let (Some(existing), Some(incoming)) = (&result.components, &document.components) {
            for (name, schema) in &incoming.schemas {
                if existing
                    .schemas
                    .get(name)
                    .is_some_and(|current| current != schema)
                {
                    return Err(format!("conflicting public schema: {name}"));
                }
            }
        }
        result.merge(document);
    }
    Ok(result)
}
