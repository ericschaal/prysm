use prysm_webos_protocol::WORKER_CONFIG_ENV;
use std::process::{Command, Stdio};

#[test]
fn worker_rejects_cli_and_missing_supervisor_contract_before_native_startup() {
    let binary = env!("CARGO_BIN_EXE_prysm-webos-capture");
    for (argument, config, expected) in [
        (Some("--help"), None, "accepts no command-line arguments"),
        (None, None, "requires supervisor configuration"),
        (
            None,
            Some(r#"{"receiver":"127.0.0.1:19400","source":"both","compression":"lz4"}"#),
            "requires a supervisor event pipe",
        ),
    ] {
        let mut command = Command::new(binary);
        command
            .env_remove(WORKER_CONFIG_ENV)
            .env_remove("PRYSM_EVENTS_FD")
            .stdin(Stdio::null());
        if let Some(argument) = argument {
            command.arg(argument);
        }
        if let Some(config) = config {
            command.env(WORKER_CONFIG_ENV, config);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{:?}",
            output.stderr
        );
    }
}
