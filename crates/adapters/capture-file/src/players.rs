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
/// `Ok` also when nothing in it is left to signal. `pid` is a child of this
/// process that it has not reaped, so the group exists and is the one it
/// started. macOS answers `EPERM` when no member could be signalled, as for
/// a group whose only members have exited and wait to be reaped; Linux
/// succeeds there.
pub(crate) fn kill_group(pid: u32) -> io::Result<()> {
    let group = i32::try_from(pid).map_err(io::Error::other)?;
    match killpg(Pid::from_raw(group), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH | Errno::EPERM) => Ok(()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    /// A player that exited before its source is closed: its group holds
    /// only a process waiting to be reaped.
    #[test]
    fn killing_a_group_whose_process_has_exited_succeeds() {
        let mut child = Command::new("true")
            .process_group(0)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        // The end of its output: the process is exiting, and is not reaped.
        let mut output = Vec::new();
        let mut stdout = child.stdout.take().unwrap();
        stdout.read_to_end(&mut output).unwrap();
        kill_group(child.id()).unwrap();
        child.wait().unwrap();
    }
}
