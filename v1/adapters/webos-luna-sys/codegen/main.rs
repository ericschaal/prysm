//! Generate `src/generated/{luna,glib}.rs` from the webOS SDK headers: only the
//! functions and types the wrapper uses, loaded at run time via `libloading`.
//!
//! ```sh
//! WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
//!   cargo run --manifest-path v1/Cargo.toml -p prysm-webos-luna-sys \
//!   --features codegen --bin generate-luna-bindings -- [--check]
//! ```
//!
//! `--check` fails when a committed file differs and changes nothing.
#![forbid(unsafe_code)]
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: generate-luna-bindings [--check]  (WEBOS_SDK must be set)";

/// `(output file, header, function allowlist, library struct)`.
const BINDINGS: [(&str, &str, &str, &str); 2] = [
    (
        "luna",
        "luna-service2/lunaservice.h",
        "LSError(Init|Free)|LS(Register|RegisterCategory|CategorySetData|Unregister|GmainAttach|MessageGetPayload|MessageGetMethod|MessageGetApplicationID|MessageGetSenderServiceName|MessageRef|MessageUnref|MessageReply|SubscriptionProcess|SubscriptionReply|SubscriptionGetHandleSubscribersCount|Call|CallCancel)",
        "Luna",
    ),
    (
        "glib",
        "glib-2.0/glib.h",
        "g_main_context_(new|unref)|g_main_loop_(new|run|quit|unref)|g_timeout_source_new|g_source_(set_callback|attach|destroy|unref)",
        "Glib",
    ),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = match args.as_slice() {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(check) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("generate-luna-bindings: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(check: bool) -> Result<(), String> {
    let sdk = std::env::var_os("WEBOS_SDK").ok_or_else(|| format!("set WEBOS_SDK; {USAGE}"))?;
    let sdk = PathBuf::from(sdk)
        .canonicalize()
        .map_err(|error| format!("WEBOS_SDK: {error}"))?;
    let sysroot = sdk.join("arm-webos-linux-gnueabi/sysroot");
    let include = sysroot.join("usr/include");
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/generated");
    let mut generated = Vec::new();
    for (name, header, functions, library) in BINDINGS {
        let bindings = bindgen::Builder::default()
            .header(include.join(header).to_string_lossy())
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
            .opaque_type("LSHandle|LSMessage|GMainLoop|GMainContext|GSource")
            .dynamic_library_name(library)
            .dynamic_link_require_all(true)
            .wrap_unsafe_ops(true)
            .rust_target(
                bindgen::RustTarget::stable(85, 0)
                    .map_err(|error| format!("Rust target: {error}"))?,
            )
            .rust_edition(bindgen::RustEdition::Edition2024)
            .formatter(bindgen::Formatter::Rustfmt)
            // ARM layout assertions cannot run in the host's stand-in tests.
            .layout_tests(false)
            .generate_comments(false)
            .generate()
            .map_err(|error| format!("generate from {header}: {error}"))?;
        generated.push((output.join(format!("{name}.rs")), bindings.to_string()));
    }
    // Every header parsed before any committed file changes.
    for (path, bindings) in generated {
        emit(&path, &bindings, check)?;
    }
    Ok(())
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
        println!("{} is current", path.display());
    } else if committed.as_deref() != Some(bindings) {
        std::fs::write(path, bindings)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        println!("wrote {}", path.display());
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
