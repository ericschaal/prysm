//! Build-only entry point for `npm run codegen`: prints the named OpenAPI
//! documents, or with `--fixtures` actual Rust serializations the frontend
//! tests validate against them.
mod fixtures;
use prysm_capture_device::DeviceSource;
use prysm_capture_remote::RemoteReceiver;
use prysm_output_wled::Wled;
use prysm_protocol::schema::{Contract, OpenApi};
use prysm_runtime::NullOutput;
use std::collections::BTreeMap;
use std::error::Error;
use std::io::{self, Write};

/// One document per module under `@prysm/contracts/`: `application` (wire
/// roots, adapter sections open) and one `host-*` document per set of hosts
/// that register the same adapter kinds, with those sections typed.
///
/// The TV hosts get a second entry, `host-webos`, registering the `webos`
/// source once that adapter provides a schema.
fn documents() -> Result<BTreeMap<&'static str, OpenApi>, Box<dyn Error>> {
    Ok(BTreeMap::from([
        (
            "application",
            Contract::new("Prysm application").document()?,
        ),
        ("host-desktop", desktop().document()?),
    ]))
}

/// The headless and TUI hosts.
fn desktop() -> Contract {
    Contract::new("Prysm desktop host")
        .source::<DeviceSource>()
        .source::<RemoteReceiver>()
        .output::<Wled>()
        .output::<NullOutput>()
}

fn main() -> Result<(), Box<dyn Error>> {
    let output = match std::env::args().nth(1).as_deref() {
        None => serde_json::to_value(documents()?)?,
        Some("--fixtures") => fixtures::serialized(),
        Some(_) => return Err("usage: prysm-export-contracts [--fixtures]".into()),
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
            ("RemoteReceiverSettings", "remote"),
            ("WledSettings", "wled"),
            ("NullSettings", "null"),
        ] {
            assert_eq!(schemas[name]["properties"]["kind"]["enum"], json!([kind]));
            assert_eq!(schemas[name]["required"][0], json!("kind"));
        }
        assert!(schemas["LedCounts"].is_object());
    }
}
