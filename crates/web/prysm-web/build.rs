use std::{error::Error, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../calibration-patterns.json");
    println!("cargo::rerun-if-changed=control-ui/public");
    // Watch source files individually so generated assets and node_modules cannot trigger rebuilds.
    for entry in std::fs::read_dir("control-ui")? {
        let path = entry?.path();
        if path.is_file() {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
    if !std::path::Path::new("control-ui/node_modules").is_dir() {
        return Err("frontend dependencies missing; run cargo xtask setup host".into());
    }
    let output = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?);
    let assets = output.join("dist");
    let status = Command::new("npm")
        .args(["run", "build", "--", "--outDir"])
        .arg(&assets)
        .arg("--emptyOutDir")
        .current_dir("control-ui")
        .status()
        .map_err(|error| format!("cannot run npm; install Node.js 24 and npm: {error}"))?;
    if !status.success() {
        return Err(format!("frontend build failed: {status}").into());
    }
    #[allow(
        clippy::unnecessary_debug_formatting,
        reason = "the path must be an escaped Rust string literal"
    )]
    let declaration =
        format!("#[derive(rust_embed::Embed)]\n#[folder = {assets:?}]\nstruct Assets;\n");
    std::fs::write(output.join("assets.rs"), declaration)?;
    Ok(())
}
