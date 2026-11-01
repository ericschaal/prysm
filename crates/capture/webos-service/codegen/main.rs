//! Generate only the ABI surface used by the service, from the TV SDK headers.
#![forbid(unsafe_code)]
use anyhow::{Context, Result, ensure};
use clap::Parser;
use std::path::{Path, PathBuf};

#[derive(Parser)]
struct Cli {
    /// Verify committed bindings without modifying them.
    #[arg(long)]
    check: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let sdk = PathBuf::from(std::env::var_os("WEBOS_SDK").context("set WEBOS_SDK")?)
        .canonicalize()
        .context("resolve WEBOS_SDK")?;
    let sysroot = sdk.join("arm-webos-linux-gnueabi/sysroot");
    let include = sysroot.join("usr/include");
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/generated");
    let mut generated = Vec::new();
    for (name, header, functions, library) in [
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
        (
            "../../../webos-capture/src/native/generated/luna",
            "luna-service2/lunaservice.h",
            "LSError(Init|Free)|LS(Register|Unregister)",
            "Luna",
        ),
    ] {
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
                bindgen::RustTarget::stable(85, 0).expect("Rust 1.85 supports edition 2024"),
            )
            .rust_edition(bindgen::RustEdition::Edition2024)
            .formatter(bindgen::Formatter::Rustfmt)
            // ARM layout assertions cannot run in the host's ABI stub tests.
            .layout_tests(false)
            .generate_comments(false)
            .generate()
            .with_context(|| format!("generate {header}"))?;
        generated.push((output.join(format!("{name}.rs")), bindings.to_string()));
    }
    // Parse every header before changing any committed output.
    if !cli.check {
        std::fs::create_dir_all(&output)?;
    }
    for (path, bindings) in generated {
        emit(&path, &bindings, cli.check)?;
    }
    Ok(())
}

fn emit(path: &Path, bindings: &str, check: bool) -> Result<()> {
    if check {
        let committed =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        ensure!(
            committed == bindings,
            "{} is stale; regenerate Luna bindings",
            path.display()
        );
    } else {
        std::fs::write(path, bindings).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn check_rejects_missing_or_stale_output_without_changing_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bindings.rs");
        assert!(super::emit(&path, "new", true).is_err());
        assert!(!path.exists());
        super::emit(&path, "old", false).unwrap();
        assert!(super::emit(&path, "new", true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
        super::emit(&path, "new", false).unwrap();
        super::emit(&path, "new", true).unwrap();
    }
}
