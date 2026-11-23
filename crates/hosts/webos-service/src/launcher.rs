//! Starting and stopping the worker process. [`ProcessLauncher`] is the real
//! one; tests use scripted fakes.
use crate::machine::Exit;
use prysm_webos_link::{Command, Event, LineReader, encode};
use prysm_webos_luna_sys::Waker;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Longest stderr line kept, in bytes; the rest of the line is dropped. A
/// cut inside a character becomes U+FFFD, so the text can be up to two bytes
/// longer.
const MAX_LOG_LINE: usize = 512;
/// How long after its exit a worker's last lines are waited for. Only a
/// process the worker left behind, holding its pipes, makes the wait last.
pub const LAST_WORDS: Duration = Duration::from_secs(1);

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
    /// Reap the process if it exited, without blocking. The exit comes once
    /// every event the worker sent is ready for [`Worker::drain`], or at the
    /// latest [`LAST_WORDS`] after the reap.
    ///
    /// # Errors
    /// `waitpid` failed.
    fn try_exit(&mut self) -> io::Result<Option<Exit>>;
    /// When [`Worker::try_exit`] returns an exit held back for the last
    /// lines, if no event comes first: [`LAST_WORDS`] after the reap.
    fn due(&self) -> Option<Instant>;
}

/// Workers as child processes with piped stdio. Spawn from the supervisor's
/// long-lived loop thread: the worker's parent-death signal follows it.
/// Each line read, and the end of the worker's event stream, wakes the loop
/// through `waker`; its exit wakes it through SIGCHLD, which the caller
/// routes there.
#[derive(Debug)]
pub struct ProcessLauncher {
    waker: Waker,
}

impl ProcessLauncher {
    #[must_use]
    pub fn new(waker: Waker) -> Self {
        Self { waker }
    }
}

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
        let events = Outputs {
            sender: sender.clone(),
            waker: self.waker.clone(),
        };
        let logs = Outputs {
            sender,
            waker: self.waker.clone(),
        };
        let reader = std::thread::Builder::new()
            .name("prysm-worker-events".into())
            .spawn(move || {
                read_events(stdout, &events);
                // Its end lets a held-back exit through.
                events.waker.wake();
            });
        let logger = std::thread::Builder::new()
            .name("prysm-worker-log".into())
            .spawn(move || read_log(stderr, &logs));
        let reader = match (reader, logger) {
            (Ok(reader), Ok(_)) => reader,
            (Err(error), _) | (_, Err(error)) => {
                abandon(child);
                return Err(error);
            }
        };
        Ok(ProcessWorker {
            stdin: child.stdin.take(),
            child: Some(child),
            outputs,
            reader: Some(reader),
            exited: None,
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

/// Where a reader thread puts what it read: the channel, then a wake.
struct Outputs {
    sender: mpsc::Sender<Output>,
    waker: Waker,
}

impl Outputs {
    /// `false` once nobody reads the channel.
    fn send(&self, output: Output) -> bool {
        let sent = self.sender.send(output).is_ok();
        self.waker.wake();
        sent
    }
}

fn read_events(stdout: impl Read, outputs: &Outputs) {
    let mut reader = LineReader::new(BufReader::new(stdout));
    loop {
        let output = match reader.read::<Event>() {
            Ok(Some(event)) => Output::Event(event),
            // EOF: the worker is exiting; its exit status says why.
            Ok(None) => return,
            Err(error) => Output::Broken(prysm_ports::chain(&error)),
        };
        let broken = matches!(output, Output::Broken(_));
        if !outputs.send(output) || broken {
            return;
        }
    }
}

fn read_log(stderr: impl Read, outputs: &Outputs) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    while let Ok(true) = read_line(&mut reader, &mut line) {
        let text = String::from_utf8_lossy(&line).trim_end().to_owned();
        if !outputs.send(Output::Log(text)) {
            return;
        }
    }
}

/// Read the next line into `line`, which is all the memory it uses: at most
/// [`MAX_LOG_LINE`] bytes are kept, the rest of a longer line is skipped.
/// `Ok(false)` at the end of the stream.
fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<bool> {
    line.clear();
    if reader
        .by_ref()
        .take(MAX_LOG_LINE as u64)
        .read_until(b'\n', line)?
        == 0
    {
        return Ok(false);
    }
    if line.last() != Some(&b'\n') {
        reader.skip_until(b'\n')?;
    }
    Ok(true)
}

#[derive(Debug)]
pub struct ProcessWorker {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    outputs: mpsc::Receiver<Output>,
    /// The event reader, until joined; it ends at the event stream's EOF.
    /// (The stderr reader is not waited for: logs are best effort, and
    /// whatever the worker starts inherits its stderr.)
    reader: Option<JoinHandle<()>>,
    /// Reaped at that time, its exit held back until the event reader ends
    /// or [`LAST_WORDS`] passed.
    exited: Option<(Exit, Instant)>,
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
        if let Some(child) = &mut self.child {
            let Some(status) = child.try_wait()? else {
                return Ok(None);
            };
            self.child = None;
            self.stdin = None;
            let exit = match (status.code(), status.signal()) {
                (Some(code), _) => Exit::Code(code),
                (None, Some(signal)) => Exit::Signal(signal),
                (None, None) => Exit::Code(-1),
            };
            self.exited = Some((exit, Instant::now()));
        }
        let Some((exit, reaped)) = self.exited else {
            return Ok(None);
        };
        if self.reader.as_ref().is_none_or(JoinHandle::is_finished) {
            // Joined, so every event it sent is visible to `drain`.
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        } else if reaped.elapsed() < LAST_WORDS {
            return Ok(None);
        }
        self.exited = None;
        Ok(Some(exit))
    }

    fn due(&self) -> Option<Instant> {
        self.exited.map(|(_, reaped)| reaped + LAST_WORDS)
    }
}

impl Drop for ProcessWorker {
    /// Abnormal teardown only (the machine reaps before letting go): SIGTERM,
    /// so the worker still runs its stop and releases the capture tap (SIGKILL
    /// inside a native call can leave the TV's capture locked until a
    /// reboot), and reap on a thread, so a process stuck in the kernel cannot
    /// block us. A worker that does not finish is bounded by its own stop
    /// deadline (see `prysm_webos_link::worker::guard_parent`).
    fn drop(&mut self) {
        // Already exited if this fails; the reap below collects it.
        let _ = self.terminate();
        if let Some(mut child) = self.child.take() {
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
