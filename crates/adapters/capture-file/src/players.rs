//! The FFmpeg children of this process that have not been reaped, so a host
//! that exits without closing its sessions still kills them. A child is
//! listed from its spawn until its reaping `wait` returned, so its process
//! ID is another process's at most for the instant before it is removed.
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use std::io;
use std::sync::{Mutex, MutexGuard, PoisonError};

static RUNNING: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn running() -> MutexGuard<'static, Vec<u32>> {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn add(pid: u32) {
    running().push(pid);
}

pub(crate) fn forget(pid: u32) {
    running().retain(|&listed| listed != pid);
}

#[cfg(test)]
pub(crate) fn listed(pid: u32) -> bool {
    running().contains(&pid)
}

/// SIGKILL to the process group `pid` leads (each FFmpeg has its own);
/// `Ok` also when nothing in it is left to signal.
pub(crate) fn kill_group(pid: u32) -> io::Result<()> {
    let group = i32::try_from(pid).map_err(io::Error::other)?;
    match killpg(Pid::from_raw(group), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(errno) => Err(errno.into()),
    }
}

/// Kill every FFmpeg this process started and has not killed yet, with its
/// process group. For a host about to exit while a session may still hold
/// its source: after a second interrupt, or after a session was abandoned at
/// its stop deadline. Does not wait for them: once the host has exited, the
/// system reaps them.
pub fn kill_remaining() {
    for pid in std::mem::take(&mut *running()) {
        if let Err(error) = kill_group(pid) {
            tracing::warn!(pid, %error, "cannot kill FFmpeg");
        }
    }
}
