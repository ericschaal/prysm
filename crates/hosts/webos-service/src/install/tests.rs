//! The installer in a temporary root, with shell scripts for `ls-control`
//! and `luna-send`.
use super::*;
use crate::identity::APP_ID;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::Instant;

struct Sandbox {
    dir: tempfile::TempDir,
    installer: FsInstaller,
}

/// Write an executable script from a child process. A write descriptor in
/// this multithreaded process would be copied into any child another test
/// forks meanwhile and stay open until that child execs, so executing the
/// script then could fail with ETXTBSY ("Text file busy").
fn script(path: &Path, body: &str) {
    let status = Command::new("/bin/sh")
        .args(["-c", "printf '#!/bin/sh\\n%s\\n' \"$1\" > \"$0\""])
        .arg(path)
        .arg(body)
        .status()
        .unwrap();
    assert!(status.success(), "{status}");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// `result` is an error that `expected` accepts; a failure shows the result.
#[track_caller]
fn assert_fails(result: Result<()>, expected: impl Fn(&InstallError) -> bool) {
    assert!(matches!(&result, Err(e) if expected(e)), "got {result:?}");
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
        id: APP_ID.into(),
        manifests: root.join("manifests.d"),
        service_dir: root.join("service"),
        supervisor: root.join("service/prysm-webos-service"),
        hook: root.join(our_hook()),
        scanner: root.join("ls-control"),
        luna_send: root.join("luna-send"),
        hook_log: root.join("setup.log"),
    };
    let mut installer = FsInstaller::new(layout);
    installer.timeout = Duration::from_secs(5);
    Sandbox { dir, installer }
}

/// Our manifest `name` (`supervisor`, `worker`), relative to the root.
fn manifest(name: &str) -> String {
    format!("manifests.d/{APP_ID}.{name}.json")
}

/// Our boot hook, relative to the root.
fn our_hook() -> String {
    format!("init.d/{APP_ID}")
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
        let manifest: Value = serde_json::from_str(&s.read(&manifest(name))).unwrap();
        assert_eq!(manifest["version"], MANIFEST_VERSION, "{name}");
    }
}

#[test]
fn install_is_idempotent_and_uninstall_removes_only_ours() {
    let mut s = sandbox();
    s.installer.install().unwrap();
    s.installer.install().unwrap();
    let supervisor: Value = serde_json::from_str(&s.read(&manifest("supervisor"))).unwrap();
    assert_eq!(supervisor["id"], format!("{APP_ID}.supervisor"));
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
    let worker: Value = serde_json::from_str(&s.read(&manifest("worker"))).unwrap();
    assert!(
        worker["roleFiles"][0]
            .as_str()
            .unwrap()
            .ends_with("worker.role.json")
    );
    let hook = s.read(&our_hook());
    assert!(hook.starts_with("#!/bin/sh\n"));
    assert!(hook.contains("prysm-webos-service' setup >"), "{hook}");
    assert_eq!(
        fs::metadata(s.path(&our_hook()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(s.read("scans"), "scan-services\nscan-services\n");

    s.installer.uninstall().unwrap();
    s.installer.uninstall().unwrap();
    assert!(!s.path(&manifest("supervisor")).exists());
    assert!(!s.path(&manifest("worker")).exists());
    assert!(!s.path(&our_hook()).exists());
    assert_eq!(s.read("scans").lines().count(), 4);
}

#[test]
fn unrelated_files_are_never_replaced_or_removed() {
    let mut s = sandbox();
    let manifest = s.path(&manifest("worker"));
    fs::write(&manifest, r#"{"id":"someone.else"}"#).unwrap();
    let unrelated =
        |e: &InstallError| matches!(e, InstallError::UnrelatedManifest(p) if *p == manifest);
    assert_fails(s.installer.install(), unrelated);
    assert_fails(s.installer.uninstall(), unrelated);
    assert_eq!(
        fs::read_to_string(&manifest).unwrap(),
        r#"{"id":"someone.else"}"#
    );
    fs::remove_file(&manifest).unwrap();

    fs::create_dir_all(s.path("init.d")).unwrap();
    fs::write(s.path(&our_hook()), "#!/bin/sh\nother\n").unwrap();
    let hook = s.path(&our_hook());
    let unrelated = |e: &InstallError| matches!(e, InstallError::UnrelatedHook(p) if *p == hook);
    assert_fails(s.installer.install(), unrelated);
    assert_fails(s.installer.uninstall(), unrelated);
    assert_eq!(s.read(&our_hook()), "#!/bin/sh\nother\n");

    fs::remove_file(s.path(&our_hook())).unwrap();
    nix::unistd::mkfifo(&s.path(&our_hook()), nix::sys::stat::Mode::S_IRUSR).unwrap();
    assert_fails(s.installer.install(), |e| {
        matches!(e, InstallError::Hook { .. })
    });
}

/// Ours is our id and role files, whatever the version: a manifest of
/// another release (an update that skipped `setup`) is replaced, by rename,
/// so an interrupted write leaves the old file.
#[test]
fn our_manifest_of_another_version_is_replaced_atomically() {
    let mut s = sandbox();
    s.installer.install().unwrap();
    let path = s.path(&manifest("supervisor"));
    let current = fs::read(&path).unwrap();
    let mut old: Value = serde_json::from_slice(&current).unwrap();
    old["version"] = "0.9.0".into();
    old["apiPermissionFiles"] = json!([]);
    let old = serde_json::to_vec(&old).unwrap();
    fs::write(&path, &old).unwrap();
    // A second name for the old file: an in-place write would change it too.
    let link = s.path("old-supervisor.json");
    fs::hard_link(&path, &link).unwrap();

    s.installer.install().unwrap();
    assert_eq!(fs::read(&path).unwrap(), current);
    assert_eq!(
        fs::read(&link).unwrap(),
        old,
        "written beside, then renamed"
    );
    let metadata = fs::metadata(&path).unwrap();
    assert_ne!(metadata.ino(), fs::metadata(&link).unwrap().ino());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o644);
    let names: Vec<_> = fs::read_dir(s.path("manifests.d"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 2, "no temporary file left: {names:?}");

    fs::remove_file(&link).unwrap();
    s.installer.install().unwrap();
    assert_eq!(fs::metadata(&path).unwrap().ino(), metadata.ino());

    let worker = s.path(&manifest("worker"));
    let mut old: Value = serde_json::from_slice(&fs::read(&worker).unwrap()).unwrap();
    old["version"] = "2.0.0".into();
    fs::write(&worker, serde_json::to_vec(&old).unwrap()).unwrap();
    s.installer.uninstall().unwrap();
    assert!(!worker.exists());
    assert!(!path.exists());
}

/// Somebody else's manifest under our file name is never ours: our id with
/// role files elsewhere is refused like another id.
#[test]
fn a_manifest_with_our_id_but_other_role_files_is_refused() {
    let mut s = sandbox();
    let path = s.path(&manifest("worker"));
    let foreign = format!(
        r#"{{"id":"{APP_ID}.worker","version":"1.0.0","roleFiles":["/elsewhere/worker.role.json"]}}"#
    );
    fs::write(&path, &foreign).unwrap();
    let unrelated =
        |e: &InstallError| matches!(e, InstallError::UnrelatedManifest(p) if *p == path);
    assert_fails(s.installer.install(), unrelated);
    assert_fails(s.installer.uninstall(), unrelated);
    assert_eq!(fs::read_to_string(&path).unwrap(), foreign);
}

/// Ours is the hook with our marker line, whatever release wrote the rest:
/// it is replaced by rename. Uninstall removes it.
#[test]
fn our_hook_of_another_release_is_replaced() {
    let mut s = sandbox();
    let hook = s.path(&our_hook());
    fs::create_dir_all(s.path("init.d")).unwrap();
    let old = format!("#!/bin/sh\n# Prysm ({APP_ID}): older text.\n/old/path setup &\n");
    fs::write(&hook, &old).unwrap();
    let link = s.path("old-hook");
    fs::hard_link(&hook, &link).unwrap();
    s.installer.install().unwrap();
    assert_eq!(s.read(&our_hook()), s.installer.layout.hook_script());
    assert_eq!(
        fs::read_to_string(&link).unwrap(),
        old,
        "replaced by rename"
    );
    assert_eq!(
        fs::metadata(&hook).unwrap().permissions().mode() & 0o777,
        0o755
    );
    fs::write(&hook, &old).unwrap();
    s.installer.uninstall().unwrap();
    assert!(!hook.exists());
}

/// A failed step still rescans: the manifests it replaced take effect.
#[test]
fn a_hook_failure_still_rescans() {
    let mut s = sandbox();
    fs::create_dir_all(s.path("init.d")).unwrap();
    fs::write(s.path(&our_hook()), "#!/bin/sh\nother\n").unwrap();
    assert_fails(s.installer.install(), |e| {
        matches!(e, InstallError::UnrelatedHook(_))
    });
    assert!(s.path(&manifest("worker")).exists());
    assert_eq!(s.read("scans"), "scan-services\n");
    assert_fails(s.installer.uninstall(), |e| {
        matches!(e, InstallError::UnrelatedHook(_))
    });
    assert_eq!(s.read("scans"), "scan-services\nscan-services\n");
}

/// A manifest that is not JSON stays refused, with the remedy in the error.
#[test]
fn a_broken_manifest_is_refused_with_the_remedy() {
    let mut s = sandbox();
    let path = s.path(&manifest("supervisor"));
    fs::write(&path, "{\"id\":").unwrap();
    let result = s.installer.install();
    assert_fails(
        result,
        |e| matches!(e, InstallError::InvalidManifest { path: p, .. } if *p == path),
    );
    let message = s.installer.install().unwrap_err().to_string();
    assert!(
        message.contains(&format!("{APP_ID}.supervisor.json")),
        "{message}"
    );
    assert!(
        message.contains("delete it, then run setup again"),
        "{message}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\"id\":");
}

#[test]
fn rescan_failures_and_hangs_are_errors() {
    let mut s = sandbox();
    script(&s.path("ls-control"), "exit 4");
    assert_fails(
        s.installer.install(),
        |e| matches!(e, InstallError::Rescan(status) if status.code() == Some(4)),
    );
    script(&s.path("ls-control"), "sleep 5");
    s.installer.timeout = Duration::from_millis(200);
    let started = Instant::now();
    assert_fails(s.installer.install(), |e| {
        matches!(e, InstallError::Timeout { .. })
    });
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn activation_asks_for_state_and_checks_the_reply() {
    let mut s = sandbox();
    s.installer.activate("app.test.service").unwrap();
    assert_eq!(
        s.read("luna-send.args").trim(),
        "-w 5000 -n 1 luna://app.test.service/getState {}"
    );
    script(
        &s.path("luna-send"),
        "echo '{\"returnValue\":false,\"errorText\":\"denied\"}'",
    );
    assert_fails(
        s.installer.activate("x"),
        |e| matches!(e, InstallError::ActivationRefused(reply) if reply["errorText"] == "denied"),
    );
    script(&s.path("luna-send"), "exit 1");
    assert_fails(
        s.installer.activate("x"),
        |e| matches!(e, InstallError::Activation(status) if status.code() == Some(1)),
    );
}

#[test]
fn activation_requires_an_explicit_success_reply() {
    assert!(activation_reply(br#"{"returnValue":true}"#).is_ok());
    assert_fails(activation_reply(b""), |e| {
        matches!(e, InstallError::InvalidReply(_))
    });
    for value in [
        b"null".as_slice(),
        b"{}",
        br#"{"returnValue":false,"errorText":"denied"}"#,
    ] {
        assert_fails(activation_reply(value), |e| {
            matches!(e, InstallError::ActivationRefused(_))
        });
    }
    assert_fails(activation_reply(&vec![b' '; MAX_REPLY + 1]), |e| {
        matches!(e, InstallError::OversizedReply)
    });
}
