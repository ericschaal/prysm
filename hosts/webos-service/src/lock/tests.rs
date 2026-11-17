//! The lock against a separate process standing in for `run` or another
//! `uninstall`: record locks belong to a process, so the holder is this test
//! binary started again to run only [`holder`].
use super::*;
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const DIR: &str = "PRYSM_TEST_LOCK_DIR";
const ROLE: &str = "PRYSM_TEST_LOCK_ROLE";
/// The holder's exit code when the lock was taken already.
const REFUSED: i32 = 3;

/// What the holder stands in for.
#[derive(Clone, Copy)]
pub(crate) enum Role {
    /// `run`, exiting on SIGTERM.
    Supervisor,
    /// `run`, ignoring SIGTERM.
    Stubborn,
    /// Another `uninstall`, holding the lock as it does.
    Uninstall,
}

impl Role {
    const ALL: [Self; 3] = [Self::Supervisor, Self::Stubborn, Self::Uninstall];

    fn name(self) -> &'static str {
        match self {
            Self::Supervisor => "supervisor",
            Self::Stubborn => "stubborn",
            Self::Uninstall => "uninstall",
        }
    }
}

/// A process holding the lock in `dir`; killed and reaped when dropped.
pub(crate) struct Holder {
    child: Child,
    dir: PathBuf,
}

impl Holder {
    /// Start it and wait (five seconds at most) until it holds the lock.
    pub(crate) fn start(dir: &Path, role: Role) -> Self {
        let mut holder = Self::spawn(dir, role);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !holder.dir.join("held").exists() {
            assert!(
                holder.child.try_wait().unwrap().is_none(),
                "the holder exited"
            );
            assert!(
                Instant::now() < deadline,
                "the holder did not take the lock"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        holder
    }

    fn spawn(dir: &Path, role: Role) -> Self {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["lock::tests::holder", "--exact", "--ignored", "--nocapture"])
            .arg("--test-threads=1")
            .env(DIR, dir)
            .env(ROLE, role.name())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        Self {
            child,
            dir: dir.to_owned(),
        }
    }

    pub(crate) fn pid(&self) -> i32 {
        i32::try_from(self.child.id()).unwrap()
    }

    /// Wait up to five seconds for it to exit.
    fn exit(&mut self) -> Option<ExitStatus> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    pub(crate) fn terminated(&self) -> bool {
        self.dir.join("terminated").exists()
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The stand-in, only when started by [`Holder`]: holds the lock for at most
/// 20 s, marking SIGTERM in `terminated`.
#[test]
#[ignore = "a helper process for the other tests"]
fn holder() {
    let Some(dir) = std::env::var_os(DIR).map(PathBuf::from) else {
        return;
    };
    let term = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, term.clone()).unwrap();
    let name = std::env::var(ROLE).unwrap();
    let role = Role::ALL
        .into_iter()
        .find(|role| role.name() == name)
        .unwrap();
    let held = match role {
        Role::Uninstall => stop(&dir, Duration::from_secs(1)).unwrap().unwrap(),
        Role::Supervisor | Role::Stubborn => match hold(&dir) {
            Ok(held) => held,
            Err(LockError::Running(_)) => std::process::exit(REFUSED),
            Err(error) => panic!("{error}"),
        },
    };
    std::fs::write(dir.join("held"), b"").unwrap();
    let exit = !matches!(role, Role::Stubborn);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if term.load(Ordering::Relaxed) {
            std::fs::write(dir.join("terminated"), b"").unwrap();
            if exit {
                drop(held);
                std::process::exit(0);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::process::exit(0);
}

#[test]
fn without_a_state_directory_nothing_runs() {
    let root = tempfile::tempdir().unwrap();
    assert!(
        stop(&root.path().join("state"), Duration::from_secs(1))
            .unwrap()
            .is_none()
    );
    assert!(!root.path().join("state").exists(), "nothing created");
}

#[test]
fn a_free_lock_is_taken_without_signalling() {
    let dir = tempfile::tempdir().unwrap();
    let held = stop(dir.path(), Duration::from_secs(1)).unwrap();
    assert!(held.is_some());
}

#[test]
fn a_second_supervisor_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let holder = Holder::start(dir.path(), Role::Supervisor);
    match hold(dir.path()) {
        Err(LockError::Running(pid)) => assert_eq!(pid, holder.pid()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn stop_terminates_the_holder_and_keeps_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let mut holder = Holder::start(dir.path(), Role::Supervisor);
    let held = stop(dir.path(), Duration::from_secs(5)).unwrap();
    assert!(held.is_some());
    assert!(holder.terminated(), "stopped with SIGTERM");
    assert!(holder.exit().is_some_and(|status| status.success()));

    // While uninstall holds it, a supervisor Luna starts meanwhile gives up.
    let mut late = Holder::spawn(dir.path(), Role::Supervisor);
    assert_eq!(late.exit().and_then(|status| status.code()), Some(REFUSED));
    drop(held);
}

#[test]
fn a_holder_ignoring_term_is_reported_stuck() {
    let dir = tempfile::tempdir().unwrap();
    let holder = Holder::start(dir.path(), Role::Stubborn);
    let started = Instant::now();
    match stop(dir.path(), Duration::from_millis(500)) {
        Err(LockError::Stuck { pid, .. }) => assert_eq!(pid, holder.pid()),
        other => panic!("{other:?}"),
    }
    assert!(started.elapsed() >= Duration::from_millis(500));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !holder.terminated() {
        assert!(Instant::now() < deadline, "no SIGTERM arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn another_uninstall_is_never_signalled() {
    let dir = tempfile::tempdir().unwrap();
    let other = Holder::start(dir.path(), Role::Uninstall);
    let started = Instant::now();
    assert!(stop(dir.path(), Duration::from_secs(5)).unwrap().is_some());
    assert!(started.elapsed() < Duration::from_secs(1), "taken at once");
    assert!(!other.terminated());
}
