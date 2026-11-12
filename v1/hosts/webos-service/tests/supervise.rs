//! The service with the real process launcher and a shell script as the
//! worker: spawn, commands, status, TERM, reap, restart, and the binaries'
//! argument handling. Time-bounded throughout.
use prysm_webos_service::install::Installer;
use prysm_webos_service::launcher::ProcessLauncher;
use prysm_webos_service::machine::Spec;
use prysm_webos_service::preferences::MemoryStore;
use prysm_webos_service::service::{Bus, Config, Service, Watch};
use prysm_webos_service::state::Flavor;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Published(Vec<Value>);

impl Bus for Published {
    fn publish(&mut self, payload: &str) {
        self.0.push(serde_json::from_str(payload).unwrap());
    }
    fn resubscribe(&mut self, _: Watch) {}
}

struct NoInstaller;

impl Installer for NoInstaller {
    fn install(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn activate(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn uninstall(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// A worker that reports one status, logs each command and stops on TERM.
const WORKER: &str = r#"#!/bin/sh
trap 'echo "TERM" >&2; exit 0' TERM
echo "args: $*" >&2
echo '{"type":"status","capturing":false,"target":"off","portal":"http://10.0.0.2:8787/","failure":null}'
exec 3<&0
while read -r line <&3; do echo "command: $line" >&2; done &
while :; do sleep 0.05; done
"#;

fn worker(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("prysm-webos");
    std::fs::write(&path, WORKER).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn until(
    service: &mut Service<ProcessLauncher, MemoryStore, NoInstaller>,
    bus: &mut Published,
    done: impl Fn(&Published, bool) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let running = service.tick(bus, Instant::now());
        if done(bus, running) {
            return;
        }
        assert!(Instant::now() < deadline, "published: {:#?}", bus.0.last());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn supervises_a_real_child_process() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        flavor: Flavor::Full,
        app_id: "io.prysm.tv".into(),
        program: worker(dir.path()),
        spec: Spec::Full {
            settings: dir.path().join("settings.json"),
            listen: "0.0.0.0:8787".parse().unwrap(),
        },
    };
    let mut service = Service::new(
        config,
        ProcessLauncher,
        MemoryStore::default(),
        NoInstaller,
        Instant::now(),
    );
    let mut bus = Published::default();
    service.power(&mut bus, r#"{"state":"Active"}"#, Instant::now());
    until(&mut service, &mut bus, |bus, _| {
        bus.0
            .last()
            .is_some_and(|state| state["portal"] == "http://10.0.0.2:8787/")
    });
    let reply = service.request(
        &mut bus,
        "setEnabled",
        "io.prysm.tv",
        r#"{"enabled":true}"#,
        Instant::now(),
    );
    assert!(reply.payload.contains(r#""enabled":true"#));
    until(&mut service, &mut bus, |bus, _| {
        bus.0.last().is_some_and(|state| {
            state["log"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap().contains(r#""target":"processed""#))
        })
    });
    let log = bus.0.last().unwrap()["log"].clone();
    assert!(log[0].as_str().unwrap().contains("--lighting off"), "{log}");

    service.power(&mut bus, r#"{"state":"Active Standby"}"#, Instant::now());
    until(&mut service, &mut bus, |bus, _| {
        bus.0
            .last()
            .is_some_and(|state| state["worker"] == "stopped")
    });
    // The next worker starts with the remembered target.
    service.power(&mut bus, r#"{"state":"Active"}"#, Instant::now());
    until(&mut service, &mut bus, |bus, _| {
        bus.0.last().is_some_and(|state| {
            state["log"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap().contains("--lighting processed"))
        })
    });
    service.shutdown(&mut bus, Instant::now());
    until(&mut service, &mut bus, |_, running| !running);
}

#[test]
fn binaries_reject_bad_arguments_and_refuse_to_run_off_the_tv() {
    for binary in [
        env!("CARGO_BIN_EXE_prysm-webos-service"),
        env!("CARGO_BIN_EXE_prysm-webos-slim-service"),
    ] {
        let output = Command::new(binary).arg("start").output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let help = Command::new(binary).arg("--help").output().unwrap();
        assert!(help.status.success());
        assert!(String::from_utf8_lossy(&help.stdout).contains("setup"));

        let sandbox = tempfile::Builder::new()
            .prefix("prysm-supervisor.")
            .tempdir()
            .unwrap();
        std::fs::set_permissions(sandbox.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = Command::new(binary)
            .args([
                "run",
                "--sandbox",
                sandbox.path().to_str().unwrap(),
                "--run-for",
                "1",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        if cfg!(target_os = "linux") {
            assert!(stderr.contains("libluna-service2"), "{stderr}");
        } else {
            assert!(stderr.contains("only on an LG webOS TV"), "{stderr}");
        }
    }
}
