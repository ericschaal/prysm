//! Starting and stopping the worker process. [`ProcessLauncher`] is the real
//! one; tests use scripted fakes.
use crate::machine::Exit;
use prysm_webos_link::{Command, Event, LineReader, encode};
use prysm_webos_luna_sys::Waker;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::mem;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::warn;

/// Longest log line the tail keeps, in bytes; the rest of the line is dropped. A
/// cut inside a character becomes U+FFFD, so the text can be up to two bytes
/// longer.
pub(crate) const MAX_LOG_LINE: usize = 512;
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
    /// Whether a worker let go before its reap (dropped) no longer exists
    /// since the last call: its late reap returned. Without blocking.
    fn let_go_reaped(&mut self) -> bool;
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
/// routes there. A worker let go says through `waker` when its late reap
/// returned.
#[derive(Debug)]
pub struct ProcessLauncher {
    waker: Waker,
    reaps: mpsc::Sender<()>,
    reaped: mpsc::Receiver<()>,
}

impl ProcessLauncher {
    #[must_use]
    pub fn new(waker: Waker) -> Self {
        let (reaps, reaped) = mpsc::channel();
        Self {
            waker,
            reaps,
            reaped,
        }
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
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            kill_and_reap(&mut child, "the worker");
            return Err(io::Error::other("worker pipes missing"));
        };
        let events = WakeSender {
            sender,
            waker: self.waker.clone(),
        };
        let logs = events.clone();
        let reader = EventReader::spawn(stdout, events);
        let logger = std::thread::Builder::new()
            .name("prysm-worker-log".into())
            .spawn(move || read_log(stderr, |line| logs.send(Output::Log(line))));
        let reader = match (reader, logger) {
            (Ok(reader), Ok(_)) => reader,
            (Err(error), _) | (_, Err(error)) => {
                kill_and_reap(&mut child, "the worker");
                return Err(error);
            }
        };
        Ok(ProcessWorker {
            outputs,
            wait_failed: false,
            reaped: WakeSender {
                sender: self.reaps.clone(),
                waker: self.waker.clone(),
            },
            stage: Stage::Running {
                child,
                stdin,
                reader,
            },
        })
    }

    fn let_go_reaped(&mut self) -> bool {
        self.reaped.try_iter().count() > 0
    }
}

/// Attempts to kill and reap a child after setup failed. Logs either error.
pub(crate) fn kill_and_reap(child: &mut Child, what: &str) {
    if let Err(error) = child.kill() {
        warn!(child = what, %error, "cannot kill");
    }
    if let Err(error) = child.wait() {
        warn!(child = what, %error, "cannot reap");
    }
}

/// Queues a notification before waking the supervisor's event loop.
#[derive(Debug, Clone)]
struct WakeSender<T> {
    sender: mpsc::Sender<T>,
    waker: Waker,
}

impl<T> WakeSender<T> {
    /// `false` once nobody reads the channel.
    fn send(&self, value: T) -> bool {
        let sent = self.sender.send(value).is_ok();
        self.waker.wake();
        sent
    }
}

fn read_events(stdout: impl Read, outputs: &WakeSender<Output>) {
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

/// Each line of `log` as the state's tail shows it, until `keep` returns
/// `false`: cut to [`MAX_LOG_LINE`], trailing whitespace trimmed.
pub(crate) fn read_log(log: impl Read, mut keep: impl FnMut(String) -> bool) {
    let mut reader = BufReader::new(log);
    let mut line = Vec::new();
    while let Ok(true) = read_line(&mut reader, &mut line) {
        if !keep(String::from_utf8_lossy(&line).trim_end().to_owned()) {
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
    /// What the readers sent; read in every stage, also after the exit.
    outputs: mpsc::Receiver<Output>,
    stage: Stage,
    /// `waitpid` failed (with ECHILD, the process is already gone).
    wait_failed: bool,
    reaped: WakeSender<()>,
}

/// Where the worker process is between its spawn and its reported exit.
#[derive(Debug)]
enum Stage {
    Running {
        child: Child,
        stdin: ChildStdin,
        reader: EventReader,
    },
    /// Reaped at `at`, its exit held back until the event reader ends or
    /// [`LAST_WORDS`] passed. Its stdin is closed.
    Reaped {
        exit: Exit,
        at: Instant,
        reader: EventReader,
    },
    /// The exit was handed over; nothing is left of the process.
    Reported,
}

/// The event reader thread and whether it has read its last event: it ends
/// at the event stream's EOF. (The stderr reader is not waited for: logs are
/// best effort, and whatever the worker starts inherits its stderr.)
#[derive(Debug)]
struct EventReader {
    thread: JoinHandle<()>,
    ended: Arc<AtomicBool>,
}

impl EventReader {
    fn spawn(stdout: ChildStdout, events: WakeSender<Output>) -> io::Result<Self> {
        let ended = Arc::new(AtomicBool::new(false));
        let completion = ended.clone();
        let thread = std::thread::Builder::new()
            .name("prysm-worker-events".into())
            .spawn(move || {
                read_events(stdout, &events);
                completion.store(true, Ordering::Release);
                events.waker.wake();
            })?;
        Ok(Self { thread, ended })
    }

    fn ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }
}

impl ProcessWorker {
    /// Does nothing after reap. A failed wait prevents further signals to the PID.
    fn signal(&self, signal: nix::sys::signal::Signal) -> io::Result<()> {
        let Stage::Running { child, .. } = &self.stage else {
            return Ok(());
        };
        if self.wait_failed {
            return Err(io::Error::other(
                "not signalled: waiting for it failed, so its pid may be another process's",
            ));
        }
        let pid = i32::try_from(child.id()).map_err(io::Error::other)?;
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal).map_err(io::Error::from)
    }
}

impl Worker for ProcessWorker {
    fn send(&mut self, command: &Command) -> io::Result<()> {
        let line = encode(command).map_err(io::Error::other)?;
        let Stage::Running { stdin, .. } = &mut self.stage else {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        };
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
        if let Stage::Running { child, .. } = &mut self.stage {
            let status = child.try_wait().inspect_err(|_| self.wait_failed = true)?;
            let Some(status) = status else {
                return Ok(None);
            };
            let exit = match (status.code(), status.signal()) {
                (Some(code), _) => Exit::Code(code),
                (None, Some(signal)) => Exit::Signal(signal),
                (None, None) => Exit::Code(-1),
            };
            // Closes stdin; the exit is handed over below if it may be.
            if let Stage::Running { reader, .. } = mem::replace(&mut self.stage, Stage::Reported) {
                self.stage = Stage::Reaped {
                    exit,
                    at: Instant::now(),
                    reader,
                };
            }
        }
        let Stage::Reaped { exit, at, reader } = &self.stage else {
            return Ok(None);
        };
        let (exit, ended) = (*exit, reader.ended());
        if !ended && at.elapsed() < LAST_WORDS {
            return Ok(None);
        }
        if let Stage::Reaped { reader, .. } = mem::replace(&mut self.stage, Stage::Reported)
            && ended
        {
            // Joined (it only returns after `ended`), so every event it sent
            // is visible to `drain`. Past `LAST_WORDS` it is left running.
            let _ = reader.thread.join();
        }
        Ok(Some(exit))
    }

    fn due(&self) -> Option<Instant> {
        match &self.stage {
            Stage::Reaped { at, .. } => Some(*at + LAST_WORDS),
            Stage::Running { .. } | Stage::Reported => None,
        }
    }
}

impl Drop for ProcessWorker {
    /// Requests termination and reaps a running child off the event-loop thread.
    fn drop(&mut self) {
        let _ = self.terminate();
        if let Stage::Running { mut child, .. } = mem::replace(&mut self.stage, Stage::Reported) {
            let reaped = self.reaped.clone();
            let reaper = std::thread::Builder::new()
                .name("prysm-worker-reaper".into())
                .spawn(move || {
                    if let Err(error) = child.wait()
                        && error.raw_os_error() != Some(nix::libc::ECHILD)
                    {
                        warn!(%error, "cannot confirm the worker was reaped");
                        return;
                    }
                    reaped.send(());
                });
            if let Err(error) = reaper {
                warn!(%error, "cannot start the worker reaper");
            }
        }
    }
}

#[cfg(test)]
mod tests;
