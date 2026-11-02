use std::{error::Error, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    // Watch source files individually so generated assets and node_modules cannot trigger rebuilds.
    for entry in std::fs::read_dir("control-ui")? {
        let path = entry?.path();
        if path.is_file() {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
    for args in [
        &["ci", "--no-audit", "--no-fund"][..],
        &["run", "build"][..],
    ] {
        let status = Command::new("npm")
            .args(args)
            .current_dir("control-ui")
            .status()
            .map_err(|error| format!("cannot run npm; install Node.js 24 and npm: {error}"))?;
        if !status.success() {
            return Err(format!("npm {} failed: {status}", args.join(" ")).into());
        }
    }
    Ok(())
}
