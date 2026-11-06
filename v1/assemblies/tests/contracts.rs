#![cfg(feature = "schema")]

use prysm_assemblies::contracts::{compose, server, webos};
use prysm_output_wled::WledSettings;
use utoipa::openapi::{
    OpenApi,
    schema::{ObjectBuilder, Type},
};

fn names(document: OpenApi) -> Vec<String> {
    document.components.unwrap().schemas.into_keys().collect()
}

#[test]
fn host_contracts_include_shared_types_and_only_their_adapters() {
    let shared = names(prysm_protocol::schema::document());
    let mut server_expected = shared.clone();
    server_expected.extend([
        "DeviceCaptureSettings".into(),
        "RemoteCaptureSettings".into(),
        "ServerCaptureSettings".into(),
        "WledSettings".into(),
    ]);
    server_expected.sort();
    let mut webos_expected = shared;
    webos_expected.extend(["TvCaptureSettings".into(), "WledSettings".into()]);
    webos_expected.sort();
    assert_eq!(names(server().unwrap()), server_expected);
    assert_eq!(names(webos().unwrap()), webos_expected);
    assert_eq!(
        names(prysm_capture_device::schema::document()),
        ["DeviceCaptureSettings"]
    );
    assert_eq!(
        names(prysm_remote_capture::schema::document()),
        ["RemoteCaptureSettings"]
    );
    assert_eq!(
        names(prysm_output_wled::schema::document()),
        ["WledSettings"]
    );
}

#[test]
fn identical_shared_schemas_can_be_reused_but_conflicting_definitions_fail() {
    let original = prysm_output_wled::schema::document();
    assert!(compose("Test", [original.clone(), original.clone()]).is_ok());
    let mut conflicting = original.clone();
    conflicting.components.as_mut().unwrap().schemas.insert(
        "WledSettings".into(),
        ObjectBuilder::new()
            .schema_type(Type::String)
            .build()
            .into(),
    );
    let result = compose("Test", [original, conflicting]);
    assert!(matches!(result, Err(message) if message.contains("WledSettings")));
}

#[test]
fn library_can_export_a_concrete_generic_without_changing_the_composer() {
    #[derive(utoipa::ToSchema)]
    struct Envelope<T> {
        #[allow(dead_code)]
        value: T,
    }
    #[derive(utoipa::OpenApi)]
    #[openapi(components(schemas(Envelope<WledSettings>)))]
    struct GenericContract;
    let document = compose(
        "Specialized test",
        [<GenericContract as utoipa::OpenApi>::openapi()],
    )
    .unwrap();
    let json = serde_json::to_value(document).unwrap();
    let value = &json["components"]["schemas"]["Envelope_WledSettings"]["properties"]["value"];
    assert_eq!(value["properties"]["address"]["type"], "string");
    assert_eq!(
        value["properties"]["leds"]["prefixItems"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}
