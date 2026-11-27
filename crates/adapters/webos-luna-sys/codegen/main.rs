//! Generate `src/generated/{luna,glib}.rs` from the webOS SDK headers: only the
//! functions and types the wrapper uses, loaded at run time via `libloading`.
//! Also generate the VTCapture exception bridge from its C++ definitions and vendor header.
//!
//! ```sh
//! WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
//!   cargo run -p prysm-webos-luna-sys \
//!   --features codegen --bin generate-luna-bindings -- [--check]
//! ```
//!
//! `--check` fails when a committed file differs and changes nothing.
#![forbid(unsafe_code)]
use clap::Parser;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Generate Luna and GLib bindings from WEBOS_SDK and the VTCapture bindings from source.
#[derive(Debug, Parser)]
struct Cli {
    /// Fail when a committed file differs; change nothing
    #[arg(long)]
    check: bool,
}

/// `(output file, headers, function allowlist, library struct)`.
const BINDINGS: [(&str, &[&str], &str, &str); 2] = [
    (
        "luna",
        &["luna-service2/lunaservice.h"],
        "LSError(Init|Free)|LS(Register|RegisterCategory|CategorySetData|Unregister|GmainAttach|MessageGetPayload|MessageGetMethod|MessageGetApplicationID|MessageGetSenderServiceName|MessageReply|SubscriptionProcess|SubscriptionReply|Call|CallCancel)",
        "Luna",
    ),
    (
        "glib",
        &["glib-2.0/glib.h"],
        "g_main_context_(new|unref)|g_main_loop_(new|run|quit|unref)|g_timeout_source_new|g_io_channel_(unix_new|set_close_on_unref|unref)|g_io_create_watch|g_source_(set_callback|attach|destroy|unref)",
        "Glib",
    ),
];

fn main() -> ExitCode {
    match run(Cli::parse().check) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("generate-luna-bindings: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(check: bool) -> Result<(), String> {
    let sdk =
        std::env::var_os("WEBOS_SDK").ok_or_else(|| "set WEBOS_SDK to the webOS SDK".to_owned())?;
    let sdk = PathBuf::from(sdk)
        .canonicalize()
        .map_err(|error| format!("WEBOS_SDK: {error}"))?;
    let sysroot = sdk.join("arm-webos-linux-gnueabi/sysroot");
    let include = sysroot.join("usr/include");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = manifest.join("src/generated");
    let mut generated = Vec::new();
    for (name, headers, functions, library) in BINDINGS {
        let bindings = headers
            .iter()
            .fold(builder()?, |builder, header| {
                builder.header(include.join(header).to_string_lossy())
            })
            .clang_args([
                "--target=armv7-unknown-linux-gnueabi".to_owned(),
                "-mcpu=cortex-a9".to_owned(),
                "-mfloat-abi=softfp".to_owned(),
                "-mfpu=neon".to_owned(),
                format!("--sysroot={}", sysroot.display()),
                format!("-I{}", include.display()),
                format!("-I{}/glib-2.0", include.display()),
                format!("-I{}/usr/lib/glib-2.0/include", sysroot.display()),
            ])
            .allowlist_function(functions)
            .detect_include_paths(false)
            .allowlist_type("LSHandle|LSMessage")
            .opaque_type("LSHandle|LSMessage|GMainLoop|GMainContext|GSource|GIOChannel")
            .dynamic_library_name(library)
            .dynamic_link_require_all(true)
            .wrap_unsafe_ops(true)
            .generate()
            .map_err(|error| format!("generate from {}: {error}", headers.join(", ")))?;
        generated.push((output.join(format!("{name}.rs")), bindings.to_string()));
    }
    let capture = manifest.join("../capture-webos-sys");
    let bridge = builder()?
        .header(capture.join("native/vtcapture.cpp").to_string_lossy())
        .clang_args(["-x", "c++", "-std=c++11"])
        .allowlist_function("prysm_vt_.*")
        // The driver is held only by pointer. Its private layout differs on the TV and host.
        .blocklist_type("VT_DRIVER_T")
        .raw_line("pub type VT_DRIVER_T = ::std::ffi::c_void;")
        .derive_default(true)
        .generate()
        .map_err(|error| format!("generate VTCapture bridge: {error}"))?;
    generated.push((
        capture.join("src/generated/vtcapture.rs"),
        bridge.to_string(),
    ));
    for (path, bindings) in generated {
        emit(&path, &bindings, check)?;
    }
    Ok(())
}

fn builder() -> Result<bindgen::Builder, String> {
    Ok(bindgen::Builder::default()
        .rust_target(bindgen::RustTarget::stable(85, 0).map_err(|error| error.to_string())?)
        .rust_edition(bindgen::RustEdition::Edition2024)
        .formatter(bindgen::Formatter::Rustfmt)
        // The host and the 32-bit TV both use the declarations.
        .layout_tests(false)
        .generate_comments(false))
}

fn emit(path: &Path, bindings: &str, check: bool) -> Result<(), String> {
    let committed = std::fs::read_to_string(path).ok();
    if check {
        if committed.as_deref() != Some(bindings) {
            return Err(format!(
                "{} is stale or missing; regenerate without --check",
                path.display()
            ));
        }
        writeln!(io::stdout().lock(), "{} is current", path.display())
            .map_err(|error| error.to_string())?;
    } else if committed.as_deref() != Some(bindings) {
        std::fs::write(path, bindings)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        writeln!(io::stdout().lock(), "wrote {}", path.display())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::emit;

    #[test]
    fn check_rejects_missing_or_stale_output_without_changing_it() {
        let directory =
            std::env::temp_dir().join(format!("luna-bindgen-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("bindings.rs");
        let _ = std::fs::remove_file(&path);
        assert!(emit(&path, "new", true).is_err());
        assert!(!path.exists());
        emit(&path, "old", false).unwrap();
        assert!(emit(&path, "new", true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
        emit(&path, "new", false).unwrap();
        emit(&path, "new", true).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
