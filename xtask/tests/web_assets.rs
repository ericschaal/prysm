//! Exercise the real build script without npm downloads or frontend mutation.
#![cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

#[test]
fn embedded_assets_use_cargos_output_and_never_install_dependencies() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project with spaces");
    let ui = root.join("control-ui");
    let output = root.join("cargo output");
    let bin = root.join("bin");
    for path in [&ui, &output, &bin] {
        fs::create_dir_all(path).unwrap();
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/web/prysm-web/build.rs");
    let executable = root.join("build-script");
    assert!(
        Command::new("rustc")
            .args(["--edition=2024"])
            .arg(script)
            .arg("-o")
            .arg(&executable)
            .status()
            .unwrap()
            .success()
    );
    let npm = bin.join("npm");
    fs::write(
        &npm,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$PRYSM_NPM_ARGS\"\nexit \"${PRYSM_NPM_EXIT:-0}\"\n",
    )
    .unwrap();
    fs::set_permissions(&npm, fs::Permissions::from_mode(0o755)).unwrap();
    let arguments = root.join("npm-args");
    let mut command = Command::new(executable);
    command
        .current_dir(&root)
        .env("OUT_DIR", &output)
        .env("PATH", &bin)
        .env("PRYSM_NPM_ARGS", &arguments);
    let missing = command.output().unwrap();
    assert!(!missing.status.success());
    assert!(
        String::from_utf8(missing.stderr)
            .unwrap()
            .contains("cargo xtask setup host")
    );
    assert!(!arguments.exists());
    fs::create_dir(ui.join("node_modules")).unwrap();
    assert!(command.output().unwrap().status.success());
    assert_eq!(
        fs::read_to_string(&arguments)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "run",
            "build",
            "--",
            "--outDir",
            output.join("dist").to_str().unwrap(),
            "--emptyOutDir"
        ]
    );
    let assets = fs::read_to_string(output.join("assets.rs")).unwrap();
    assert!(assets.contains(output.join("dist").to_str().unwrap()));
    assert!(!ui.join("dist").exists());
    assert!(
        !command
            .env("PRYSM_NPM_EXIT", "1")
            .output()
            .unwrap()
            .status
            .success()
    );
}
