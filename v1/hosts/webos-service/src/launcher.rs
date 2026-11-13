//! Starting and stopping the worker process. [`ProcessLauncher`] is the real
//! one; tests use scripted fakes.
use crate::machine::Exit;
use prysm_webos_link::{Command, Event, LineReader, encode};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;

/// Longest stderr line kept; the rest of the line is dropped.
const MAX_LOG_LINE: usize = 512;

/// Something a worker said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Event(Event),
    /// A stderr line.
    Log(String),
    /// The event stream broke; nothing more will be read from it.
    Broken(String),
}

pub trait Launcher {
    type Worker: Worker;
    /// # Errors
    /// The process could not be started.
    fn spawn(&mut self, program: &Path, args: &[String]) -> io::Result<Self::Worker>;
}

/// One started worker process.
pub trait Worker {
    /// # Errors
    /// The worker's stdin is closed.
    fn send(&mut self, command: &Command) -> io::Result<()>;
    /// SIGTERM.
    ///
    /// # Errors
    /// The signal could not be sent.
    fn terminate(&mut self) -> io::Result<()>;
    /// SIGKILL.
    ///
    /// # Errors
    /// The signal could not be sent.
    fn kill(&mut self) -> io::Result<()>;
    /// Everything said since the last call, without blocking.
    fn drain(&mut self) -> Vec<Output>;
    /// Reap the process if it exited, without blocking.
    ///
    /// # Errors
    /// `waitpid` failed.
    fn try_exit(&mut self) -> io::Result<Option<Exit>>;
}

/// Workers as child processes with piped stdio. Spawn from the supervisor's
/// long-lived loop thread: the worker's parent-death signal follows it.
#[derive(Debug, Default)]
pub struct ProcessLauncher;

impl Launcher for ProcessLauncher {
    type Worker = ProcessWorker;

    fn spawn(&mut self, program: &Path, args: &[String]) -> io::Result<ProcessWorker> {
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let (sender, outputs) = mpsc::channel();
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            abandon(child);
            return Err(io::Error::other("worker pipes missing"));
        };
        let events = sender.clone();
        let reader = std::thread::Builder::new()
            .name("prysm-worker-events".into())
            .spawn(move || read_events(stdout, &events));
        let logger = std::thread::Builder::new()
            .name("prysm-worker-log".into())
            .spawn(move || read_log(stderr, &sender));
        if let Err(error) = reader.and(logger) {
            abandon(child);
            return Err(error);
        }
        Ok(ProcessWorker {
            stdin: child.stdin.take(),
            child: Some(child),
            outputs,
        })
    }
}

/// Kill and reap a worker that could not be set up. Its pipes are not read
/// yet, so it cannot block on them; a kill that fails (it already exited)
/// leaves only the reap.
fn abandon(mut child: Child) {
    if let Err(error) = child.kill() {
        eprintln!("prysm-webos-service: cannot kill the worker: {error}");
    }
    if let Err(error) = child.wait() {
        eprintln!("prysm-webos-service: cannot reap the worker: {error}");
    }
}

fn read_events(stdout: impl Read, outputs: &mpsc::Sender<Output>) {
    let mut reader = LineReader::new(BufReader::new(stdout));
    loop {
        let output = match reader.read::<Event>() {
            Ok(Some(event)) => Output::Event(event),
            // EOF: the worker is exiting; its exit status says why.
            Ok(None) => return,
            Err(error) => Output::Broken(prysm_ports::chain(&error)),
        };
        let broken = matches!(output, Output::Broken(_));
        if outputs.send(output).is_err() || broken {
            return;
        }
    }
}

fn read_log(stderr: impl Read, outputs: &mpsc::Sender<Output>) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader)
            .take(MAX_LOG_LINE as u64)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if line.last() != Some(&b'\n') {
            // Drop the rest of an overlong line.
            let mut rest = Vec::new();
            if reader.read_until(b'\n', &mut rest).is_err() {
                return;
            }
        }
        let text = String::from_utf8_lossy(&line).trim_end().to_owned();
        if outputs.send(Output::Log(text)).is_err() {
            return;
        }
    }
}

#[derive(Debug)]
pub struct ProcessWorker {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    outputs: mpsc::Receiver<Output>,
}

impl ProcessWorker {
    fn signal(&self, signal: nix::sys::signal::Signal) -> io::Result<()> {
        let Some(child) = &self.child else {
            return Ok(());
        };
        let pid = i32::try_from(child.id()).map_err(io::Error::other)?;
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal).map_err(io::Error::from)
    }
}

impl Worker for ProcessWorker {
    fn send(&mut self, command: &Command) -> io::Result<()> {
        let line = encode(command).map_err(io::Error::other)?;
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
        stdin.write_all(&line).and_then(|()| stdin.flush())
    }

    fn terminate(&mut self) -> io::Result<()> {
        self.signal(nix::sys::signal::Signal::SIGTERM)
    }

    fn kill(&mut self) -> io::Result<()> {
        self.signal(nix::sys::signal::Signal::SIGKILL)
    }

    fn drain(&mut self) -> Vec<Output> {
        self.outputs.try_iter().collect()
    }

    fn try_exit(&mut self) -> io::Result<Option<Exit>> {
        let Some(child) = &mut self.child else {
            return Ok(None);
        };
        let Some(status) = child.try_wait()? else {
            return Ok(None);
        };
        self.child = None;
        self.stdin = None;
        Ok(Some(match (status.code(), status.signal()) {
            (Some(code), _) => Exit::Code(code),
            (None, Some(signal)) => Exit::Signal(signal),
            (None, None) => Exit::Code(-1),
        }))
    }
}

impl Drop for ProcessWorker {
    /// Abnormal teardown only (the machine reaps before letting go): kill and
    /// reap on a thread, so a process stuck in the kernel cannot block us.
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Already exited if this fails; the reap below collects it.
            let _ = child.kill();
            let reaper = std::thread::Builder::new()
                .name("prysm-worker-reaper".into())
                .spawn(move || {
                    if let Err(error) = child.wait() {
                        eprintln!("prysm-webos-service: cannot reap the worker: {error}");
                    }
                });
            if let Err(error) = reaper {
                eprintln!("prysm-webos-service: cannot start the worker reaper: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests;
