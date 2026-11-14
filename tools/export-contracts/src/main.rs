//! Build-only entry point for `npm run codegen`: prints the named OpenAPI
//! documents, or with `--fixtures` actual Rust serializations the frontend
//! tests validate against them.
mod fixtures;
use clap::Parser;
use prysm_capture_device::DeviceSource;
use prysm_capture_file::FileSource;
use prysm_capture_remote::RemoteReceiver;
use prysm_capture_webos::WebosCapture;
use prysm_output_wled::Wled;
use prysm_protocol::schema::{Contract, OpenApi};
use prysm_runtime::NullOutput;
use std::collections::BTreeMap;
use std::error::Error;
use std::io::{self, Write};

/// One document per module under `@prysm/contracts/`: `application` (wire
/// roots, adapter sections open), one `host-*` document per set of hosts
/// that register the same adapter kinds, with those sections typed, and
/// `luna`, the TV supervisor's Luna API.
fn documents() -> Result<BTreeMap<&'static str, OpenApi>, Box<dyn Error>> {
    Ok(BTreeMap::from([
        (
            "application",
            Contract::new("Prysm application").document()?,
        ),
        ("host-desktop", desktop().document()?),
        ("host-webos", webos().document()?),
        ("luna", prysm_webos_service::schema::document()),
    ]))
}

/// The headless and TUI hosts.
fn desktop() -> Contract {
    Contract::new("Prysm desktop host")
        .source::<DeviceSource>()
        .source::<FileSource>()
        .source::<RemoteReceiver>()
        .output::<Wled>()
        .output::<NullOutput>()
}

/// The full TV worker (`prysm-webos`); the slim worker has no portal.
fn webos() -> Contract {
    Contract::new("Prysm webOS host")
        .source::<WebosCapture>()
        .output::<Wled>()
}

/// Prints the OpenAPI documents the frontend generates its types from.
#[derive(Debug, Parser)]
struct Cli {
    /// Print Rust serializations the frontend tests validate instead
    #[arg(long)]
    fixtures: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let output = if Cli::parse().fixtures {
        fixtures::serialized()
    } else {
        serde_json::to_value(documents()?)?
    };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &output)?;
    writeln!(stdout)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn desktop_document_types_every_registered_kind() {
        let document = serde_json::to_value(desktop().document().unwrap()).unwrap();
        let schemas = &document["components"]["schemas"];
        let refs = |section: &str| -> Vec<Value> {
            schemas[section]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .map(|variant| variant["$ref"].clone())
                .collect()
        };
        assert_eq!(
            refs("CaptureSettings"),
            [
                json!("#/components/schemas/DeviceSettings"),
                json!("#/components/schemas/FileSettings"),
                json!("#/components/schemas/RemoteReceiverSettings"),
            ]
        );
        assert_eq!(
            refs("OutputSettings"),
            [
                json!("#/components/schemas/WledSettings"),
                json!("#/components/schemas/NullSettings"),
            ]
        );
        for (name, kind) in [
            ("DeviceSettings", "device"),
            ("FileSettings", "file"),
            ("RemoteReceiverSettings", "remote"),
            ("WledSettings", "wled"),
            ("NullSettings", "null"),
        ] {
            assert_eq!(schemas[name]["properties"]["kind"]["enum"], json!([kind]));
            assert_eq!(schemas[name]["required"][0], json!("kind"));
        }
        assert!(schemas["LedCounts"].is_object());
    }

    #[test]
    fn webos_document_types_what_the_tv_host_registers() {
        let document = serde_json::to_value(webos().document().unwrap()).unwrap();
        let schemas = &document["components"]["schemas"];
        assert_eq!(
            schemas["CaptureSettings"]["oneOf"],
            json!([{ "$ref": "#/components/schemas/WebosSettings" }])
        );
        assert_eq!(
            schemas["OutputSettings"]["oneOf"],
            json!([{ "$ref": "#/components/schemas/WledSettings" }])
        );
        let webos = &schemas["WebosSettings"];
        assert_eq!(webos["properties"]["kind"]["enum"], json!(["webos"]));
        assert_eq!(webos["required"], json!(["kind"]));
        assert_eq!(webos["properties"]["fps"]["maximum"], json!(60));
    }
}
