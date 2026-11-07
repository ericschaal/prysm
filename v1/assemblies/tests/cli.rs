#![cfg(any(feature = "headless", feature = "tui"))]

use std::process::Command;

const BINARIES: &[(&str, &str)] = &[
    #[cfg(feature = "headless")]
    ("prysm-headless", env!("CARGO_BIN_EXE_prysm-headless")),
    #[cfg(feature = "tui")]
    ("prysm-tui", env!("CARGO_BIN_EXE_prysm-tui")),
];

#[test]
fn each_binary_describes_its_own_composition_without_acquiring_a_terminal() {
    for (name, binary) in BINARIES {
        let output = Command::new(binary).arg("--help").output().unwrap();
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains(&format!("usage: {name} <settings.json> [--start]")));
        assert!(!help.contains("--terminal"));
        assert!(!help.contains('\x1b'));
        assert!(output.stderr.is_empty());
        let expected = if *name == "prysm-headless" {
            "WLED"
        } else {
            "interactive"
        };
        assert!(help.contains(expected));
    }
}

#[test]
fn invalid_arguments_fail_before_settings_or_owner_initialization() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("unused.json");
    for (_, binary) in BINARIES {
        for args in [
            vec![],
            vec!["--terminal"],
            vec![settings.to_str().unwrap(), "--terminal"],
            vec![settings.to_str().unwrap(), "--start", "--start"],
            vec![settings.to_str().unwrap(), "unexpected"],
        ] {
            let output = Command::new(binary).args(args).output().unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            let error = String::from_utf8(output.stderr).unwrap();
            assert!(error.contains("usage:"));
            assert!(!error.contains('\x1b'));
            assert!(!settings.exists());
        }
    }
}

#[cfg(feature = "headless")]
#[test]
fn headless_example_uses_wled_settings() {
    use prysm_assemblies::{DeviceCaptureSettings, WledSettings};
    use prysm_protocol::settings::SettingsDocument;
    let document: SettingsDocument<DeviceCaptureSettings, WledSettings> =
        serde_json::from_str(include_str!("../examples/headless.json")).unwrap();
    let _: prysm_contract::application::Settings<DeviceCaptureSettings, WledSettings> =
        document.try_into().unwrap();
    assert!(
        serde_json::from_str::<SettingsDocument<DeviceCaptureSettings, WledSettings>>(
            include_str!("../examples/terminal.json")
        )
        .is_err()
    );
}

#[cfg(feature = "tui")]
#[test]
fn tui_examples_use_terminal_settings() {
    use prysm_assemblies::DeviceCaptureSettings;
    use prysm_contract::application::Settings;
    use prysm_output_terminal::TerminalSettings;
    use prysm_protocol::settings::SettingsDocument;
    for json in [
        include_str!("../examples/terminal.json"),
        include_str!("../examples/prysm-webcam-terminal.json"),
    ] {
        let document: SettingsDocument<DeviceCaptureSettings, TerminalSettings> =
            serde_json::from_str(json).unwrap();
        let _: Settings<DeviceCaptureSettings, TerminalSettings> = document.try_into().unwrap();
    }
    assert!(
        serde_json::from_str::<SettingsDocument<DeviceCaptureSettings, TerminalSettings>>(
            include_str!("../examples/headless.json")
        )
        .is_err()
    );
}
