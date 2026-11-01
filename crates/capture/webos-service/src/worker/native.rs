//! Fork/exec boundary. The safe caller only supplies a Command, never descriptors.
use anyhow::Result;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use std::{
    fs::File,
    os::{
        fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    process::{Child, Command},
};

pub(super) fn nonblocking(file: &impl AsFd) -> Result<()> {
    let flags = OFlag::from_bits_truncate(fcntl(file, FcntlArg::F_GETFL)?);
    fcntl(file, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    Ok(())
}

pub(super) fn spawn(mut command: Command) -> Result<(Child, File)> {
    #[cfg(target_os = "linux")]
    let (reader, original) = nix::unistd::pipe2(OFlag::O_CLOEXEC)?;
    #[cfg(not(target_os = "linux"))]
    let (reader, original) = nix::unistd::pipe()?;
    fcntl(&reader, FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC))?;
    fcntl(&original, FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC))?;
    let duplicate = fcntl(&original, FcntlArg::F_DUPFD_CLOEXEC(4))?;
    // SAFETY: fcntl returned a fresh, owned descriptor. Only this OwnedFd closes it.
    let writer = unsafe { OwnedFd::from_raw_fd(duplicate) };
    drop(original);
    let descriptor = writer.as_raw_fd();
    #[cfg(target_os = "linux")]
    let parent = std::process::id();
    command.env("PRYSM_EVENTS_FD", "3");
    // SAFETY: after fork the closure calls only async-signal-safe libc operations.
    // The parent retains writer through spawn; dup2 installs a non-CLOEXEC FD 3.
    // No Rust allocation, locks, destructors or captured references run in the child.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(descriptor, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                // The worker cannot outlive a crashed supervisor. Recheck PPID to close
                // the race where its parent exits before PR_SET_PDEATHSIG is installed.
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid().cast_unsigned() != parent {
                    libc::_exit(1);
                }
            }
            Ok(())
        });
    }
    let events = File::from(reader);
    nonblocking(&events)?;
    let child = command.spawn()?;
    Ok((child, events))
}
