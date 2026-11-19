//! The installer in a temporary root, with shell scripts for `ls-control`
//! and `luna-send`.
use super::*;

struct Sandbox {
    dir: tempfile::TempDir,
    installer: FsInstaller,
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn sandbox() -> Sandbox {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("manifests.d")).unwrap();
    let scans = root.join("scans");
    script(
        &root.join("ls-control"),
        &format!("echo \"$@\" >> '{}'", scans.display()),
    );
    script(
        &root.join("luna-send"),
        "echo \"$@\" > \"$0.args\"; echo '{\"returnValue\":true}'",
    );
    let layout = Layout {
        id: "io.prysm.tv".into(),
        manifests: root.join("manifests.d"),
        service_dir: root.join("service"),
        supervisor: root.join("service/prysm-webos-service"),
        hook: root.join("init.d/io.prysm.tv"),
        scanner: root.join("ls-control"),
        luna_send: root.join("luna-send"),
        hook_log: root.join("setup.log"),
    };
    let mut installer = FsInstaller::new(layout);
    installer.timeout = Duration::from_secs(5);
    Sandbox { dir, installer }
}

impl Sandbox {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap()
    }
}

/// One version to bump: the product's (VERSION), without a pre-release
/// suffix, as the packaged appinfo.json has it.
#[test]
fn manifests_carry_the_product_version() {
    let version = prysm_version::VERSION;
    let rest = version
        .strip_prefix(MANIFEST_VERSION)
        .expect("the product version starts with it");
    assert!(rest.is_empty() || rest.starts_with(['-', '+']), "{version}");
    let parts: Vec<&str> = MANIFEST_VERSION.split('.').collect();
    assert_eq!(parts.len(), 3, "{MANIFEST_VERSION}");
    assert!(parts.iter().all(|part| part.parse::<u32>().is_ok()));

    let mut s = sandbox();
    s.installer.install().unwrap();
    for name in ["supervisor", "worker"] {
        let manifest: Value =
            serde_json::from_str(&s.read(&format!("manifests.d/io.prysm.tv.{name}.json"))).unwrap();
        assert_eq!(manifest["version"], MANIFEST_VERSION, "{name}");
    }
}

#[test]
fn install_is_idempotent_and_uninstall_removes_only_ours() {
    let mut s = sandbox();
    s.installer.install().unwrap();
    s.installer.install().unwrap();
    let supervisor: Value =
        serde_json::from_str(&s.read("manifests.d/io.prysm.tv.supervisor.json")).unwrap();
    assert_eq!(supervisor["id"], "io.prysm.tv.supervisor");
    assert!(
        supervisor["roleFiles"][0]
            .as_str()
            .unwrap()
            .ends_with("service/supervisor.role.json")
    );
    assert_eq!(
        supervisor["apiPermissionFiles"].as_array().unwrap().len(),
        1
    );
    let worker: Value =
        serde_json::from_str(&s.read("manifests.d/io.prysm.tv.worker.json")).unwrap();
    assert!(
        worker["roleFiles"][0]
            .as_str()
            .unwrap()
            .ends_with("worker.role.json")
    );
    let hook = s.read("init.d/io.prysm.tv");
    assert!(hook.starts_with("#!/bin/sh\n"));
    assert!(hook.contains("prysm-webos-service' setup >"), "{hook}");
    assert_eq!(
        fs::metadata(s.path("init.d/io.prysm.tv"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(s.read("scans"), "scan-services\nscan-services\n");

    s.installer.uninstall().unwrap();
    s.installer.uninstall().unwrap();
    assert!(!s.path("manifests.d/io.prysm.tv.supervisor.json").exists());
    assert!(!s.path("manifests.d/io.prysm.tv.worker.json").exists());
    assert!(!s.path("init.d/io.prysm.tv").exists());
    assert_eq!(s.read("scans").lines().count(), 4);
}

#[test]
fn unrelated_files_are_never_replaced_or_removed() {
    let mut s = sandbox();
    let manifest = s.path("manifests.d/io.prysm.tv.worker.json");
    fs::write(&manifest, r#"{"id":"someone.else"}"#).unwrap();
    assert!(
        s.installer
            .install()
            .unwrap_err()
            .to_string()
            .contains("unrelated Luna manifest")
    );
    assert!(
        s.installer
            .uninstall()
            .unwrap_err()
            .to_string()
            .contains("unrelated Luna manifest")
    );
    assert_eq!(
        fs::read_to_string(&manifest).unwrap(),
        r#"{"id":"someone.else"}"#
    );
    fs::remove_file(&manifest).unwrap();

    fs::create_dir_all(s.path("init.d")).unwrap();
    fs::write(s.path("init.d/io.prysm.tv"), "#!/bin/sh\nother\n").unwrap();
    assert!(
        s.installer
            .install()
            .unwrap_err()
            .to_string()
            .contains("unrelated boot hook")
    );
    assert!(
        s.installer
            .uninstall()
            .unwrap_err()
            .to_string()
            .contains("unrelated boot hook")
    );
    assert_eq!(s.read("init.d/io.prysm.tv"), "#!/bin/sh\nother\n");

    fs::remove_file(s.path("init.d/io.prysm.tv")).unwrap();
    nix::unistd::mkfifo(&s.path("init.d/io.prysm.tv"), nix::sys::stat::Mode::S_IRUSR).unwrap();
    assert!(
        s.installer.install().is_err(),
        "a FIFO is refused without blocking"
    );
}

#[test]
fn rescan_failures_and_hangs_are_errors() {
    let mut s = sandbox();
    script(&s.path("ls-control"), "exit 4");
    assert!(
        s.installer
            .install()
            .unwrap_err()
            .to_string()
            .contains("rescan failed")
    );
    script(&s.path("ls-control"), "sleep 5");
    s.installer.timeout = Duration::from_millis(200);
    let started = Instant::now();
    assert!(
        s.installer
            .install()
            .unwrap_err()
            .to_string()
            .contains("did not finish")
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn activation_asks_for_state_and_checks_the_reply() {
    let mut s = sandbox();
    s.installer.activate("io.prysm.tv.service").unwrap();
    assert_eq!(
        s.read("luna-send.args").trim(),
        "-w 5000 -n 1 luna://io.prysm.tv.service/getState {}"
    );
    script(
        &s.path("luna-send"),
        "echo '{\"returnValue\":false,\"errorText\":\"denied\"}'",
    );
    assert!(
        s.installer
            .activate("x")
            .unwrap_err()
            .to_string()
            .contains("denied")
    );
    script(&s.path("luna-send"), "exit 1");
    assert!(s.installer.activate("x").is_err());
}

#[test]
fn activation_requires_an_explicit_success_reply() {
    assert!(activation_reply(br#"{"returnValue":true}"#).is_ok());
    for value in [
        b"".as_slice(),
        b"null",
        b"{}",
        br#"{"returnValue":false,"errorText":"denied"}"#,
    ] {
        assert!(activation_reply(value).is_err());
    }
    assert!(activation_reply(&vec![b' '; MAX_REPLY + 1]).is_err());
}
