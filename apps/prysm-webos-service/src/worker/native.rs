//! Fork/exec boundary. The safe caller only supplies a Command, never descriptors.
use anyhow::Result;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use std::{
    fs::File,
    os::{
        fd::{AsFd, AsRawFd},
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
    let (reader, writer) = nix::unistd::pipe2(OFlag::O_CLOEXEC)?;
    #[cfg(not(target_os = "linux"))]
    let (reader, writer) = nix::unistd::pipe()?;
    fcntl(&reader, FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC))?;
    fcntl(&writer, FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC))?;
    let descriptor = writer.as_raw_fd();
    #[cfg(target_os = "linux")]
    let parent = nix::unistd::getpid();
    command.env("PRYSM_EVENTS_FD", descriptor.to_string());
    // SAFETY: after fork the closure calls only async-signal-safe libc operations.
    // The parent retains writer through spawn; only the child clears CLOEXEC.
    // No Rust allocation, locks, destructors or captured references run in the child.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(descriptor, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                // The worker cannot outlive a crashed supervisor. Recheck PPID to close
                // the race where its parent exits before PR_SET_PDEATHSIG is installed.
                nix::sys::prctl::set_pdeathsig(nix::sys::signal::Signal::SIGKILL)?;
                if nix::unistd::getppid() != parent {
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
