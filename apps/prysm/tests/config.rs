use prysm_contract::Config;

#[test]
fn check_validates_the_executables_output_support() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_prysm"))
        .args([
            "--config",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../prysm-egui.toml"),
            "check",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.success(), cfg!(feature = "desktop"));
    if !cfg!(feature = "desktop") {
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("--features desktop")
        );
        let startup = std::process::Command::new(env!("CARGO_BIN_EXE_prysm"))
            .args([
                "--config",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../../prysm-egui.toml"),
            ])
            .output()
            .unwrap();
        assert!(!startup.status.success());
        assert!(
            String::from_utf8(startup.stderr)
                .unwrap()
                .contains("--features desktop")
        );
    }
}

#[test]
fn loads_current_config_and_rejects_invalid_output() {
    let tagged = include_str!("../../../prysm.toml");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prysm.toml");
    std::fs::write(&path, tagged).unwrap();
    let config = prysm::config::load(&path).unwrap();
    assert_eq!(config, toml::from_str::<Config>(tagged).unwrap());
    let saved = toml::to_string(&config).unwrap();
    assert!(saved.contains("type = \"wled\""));
    std::fs::write(&path, saved).unwrap();
    assert_eq!(prysm::config::load(&path).unwrap(), config);

    let mut invalid_rate = toml::Value::try_from(&config).unwrap();
    invalid_rate["input"]["refresh_rate_hz"] = toml::Value::Integer(0);
    for invalid in [
        tagged.replace("type = \"wled\"\n", ""),
        tagged.replace("type = \"wled\"", "type = \"unknown\""),
        toml::to_string(&invalid_rate).unwrap(),
        tagged.replace("[output]\n", "[output]\ntypo = true\n"),
    ] {
        std::fs::write(&path, invalid).unwrap();
        assert!(prysm::config::load(&path).is_err());
    }
}

#[test]
fn loading_reports_file_errors_and_validates_application_settings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prysm.toml");
    let error = prysm::config::load(&path).unwrap_err();
    assert!(error.to_string().contains("cannot read"));
    assert!(error.to_string().contains(path.to_str().unwrap()));
    std::fs::write(&path, "invalid toml = [").unwrap();
    let error = prysm::config::load(&path).unwrap_err();
    assert!(error.to_string().contains("invalid configuration"));
    let mut config: Config = toml::from_str(include_str!("../../../prysm.toml")).unwrap();
    config.web.allowed_hosts.clear();
    std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
    assert!(prysm::config::load(&path).is_err());
}
