//! One supervisor per identity, and stopping it from the `uninstall`
//! subcommand.
//!
//! `run` holds a POSIX write lock on `supervisor.lock` in the private state
//! directory for the life of the process. `uninstall` asks for a read lock;
//! while `run` holds the write lock, the kernel names that process
//! (`F_GETLK`), `uninstall` sends it SIGTERM and waits until it can take the
//! read lock, and then holds it while it deregisters, so a supervisor that
//! Luna activates meanwhile exits before it starts a worker.
//!
//! A kernel lock rather than a pid file or a `/proc` scan: the kernel names
//! the live holder, so a stale or recycled pid is never signalled. Only `run`
//! of this identity takes the write lock (`setup` and other sandboxes run the
//! same binary but not with this state directory, and a read lock conflicts
//! only with a write lock), so the process signalled is always this
//! identity's supervisor. Record locks are not inherited by the worker, and
//! the file is close-on-exec. Only `fcntl` locks are used: on Linux
//! `F_GETLK` does not see `flock` locks.
use crate::machine::KILL_AFTER;
use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::libc;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::fs::File;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The lock file in the state directory.
pub const FILE: &str = "supervisor.lock";
/// How long `uninstall` waits for a running supervisor to exit after SIGTERM:
/// its worker's TERM-to-KILL window, then the reap and its own exit. A worker
/// that survives SIGKILL (stuck in the kernel) keeps its supervisor waiting
/// to reap it; `uninstall` then gives up.
pub const STOP_BOUND: Duration = KILL_AFTER.checked_add(Duration::from_secs(4)).unwrap();
const POLL: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("supervisor lock {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("a supervisor of this identity is already running (pid {0})")]
    Running(i32),
    #[error("the kernel does not name the process holding the supervisor lock")]
    UnknownHolder,
    #[error("cannot signal the running supervisor (pid {pid})")]
    Signal {
        pid: i32,
        #[source]
        source: Errno,
    },
    #[error("the running supervisor (pid {pid}) did not exit within {} s", bound.as_secs())]
    Stuck { pid: i32, bound: Duration },
}

/// The held lock; released when dropped (or when the process ends).
#[derive(Debug)]
pub struct Held {
    _file: File,
}

/// Take the lock for `run`.
///
/// # Errors
/// [`LockError::Running`] while another supervisor of this identity runs (or
/// `uninstall` is deregistering it), or the file cannot be opened or locked.
pub fn hold(state_dir: &Path) -> Result<Held, LockError> {
    let path = state_dir.join(FILE);
    let failed = |source| LockError::Io {
        path: path.clone(),
        source,
    };
    let file = open(&path).map_err(failed)?;
    match take(&file, Taker::Run).map_err(failed)? {
        None => Ok(Held { _file: file }),
        Some(pid) => Err(LockError::Running(pid)),
    }
}

/// Stop the running supervisor of this identity, if any, and hold the lock
/// (a read lock: concurrent uninstalls never signal each other).
/// `None`: there is no state directory, so no supervisor ever ran.
///
/// # Errors
/// The supervisor cannot be signalled or is still running after `bound`, or
/// the file cannot be opened or locked.
pub fn stop(state_dir: &Path, bound: Duration) -> Result<Option<Held>, LockError> {
    let path = state_dir.join(FILE);
    let failed = |source| LockError::Io {
        path: path.clone(),
        source,
    };
    let file = match open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failed(error)),
    };
    let deadline = Instant::now() + bound;
    let mut signalled = None;
    loop {
        let Some(pid) = take(&file, Taker::Uninstall).map_err(failed)? else {
            if let Some(pid) = signalled {
                eprintln!("prysm-webos-service: the supervisor (pid {pid}) exited");
            }
            return Ok(Some(Held { _file: file }));
        };
        // A different holder is a supervisor started meanwhile: stop it too.
        if signalled != Some(pid) {
            if pid <= 0 {
                return Err(LockError::UnknownHolder);
            }
            eprintln!(
                "prysm-webos-service: stopping the running supervisor (pid {pid}) with SIGTERM"
            );
            match kill(Pid::from_raw(pid), Signal::SIGTERM) {
                // Exited between the probe and the signal; the next take sees it.
                Ok(()) | Err(Errno::ESRCH) => {}
                Err(source) => return Err(LockError::Signal { pid, source }),
            }
            signalled = Some(pid);
        }
        if Instant::now() >= deadline {
            return Err(LockError::Stuck { pid, bound });
        }
        std::thread::sleep(POLL);
    }
}

fn open(path: &Path) -> io::Result<File> {
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(path)
}

/// Who takes the lock.
#[derive(Debug, Clone, Copy)]
enum Taker {
    /// `run`: a write lock, refused while anyone holds the file.
    Run,
    /// `uninstall`: a read lock, refused only while `run` holds it.
    Uninstall,
}

/// A lock on the whole file.
fn whole_file(taker: Taker) -> libc::flock {
    let kind = match taker {
        Taker::Run => libc::F_WRLCK,
        Taker::Uninstall => libc::F_RDLCK,
    };
    libc::flock {
        l_type: kind as libc::c_short,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    }
}

/// Take the lock: `None` once taken, else the pid the kernel names as the
/// holder of a conflicting one.
fn take(file: &File, taker: Taker) -> io::Result<Option<i32>> {
    loop {
        match fcntl(file, FcntlArg::F_SETLK(&whole_file(taker))) {
            Ok(_) => return Ok(None),
            Err(Errno::EAGAIN | Errno::EACCES) => {}
            Err(error) => return Err(error.into()),
        }
        let mut probe = whole_file(taker);
        fcntl(file, FcntlArg::F_GETLK(&mut probe))?;
        // Unlocked between the two calls: try again.
        if probe.l_type != libc::F_UNLCK as libc::c_short {
            return Ok(Some(probe.l_pid));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
