//! One running FFmpeg child. The session thread polls the child's stdout and
//! stderr with a deadline (no reading thread): stdout carries whole RGB24
//! frames, stderr is drained into a bounded tail that failures carry. A
//! thread blocked in the child's `wait` reports its exit, which the session
//! thread awaits with a deadline.
//!
//! FFmpeg runs in a process group of its own, so a Ctrl-C on the terminal
//! reaches only the host, which stops FFmpeg itself by killing that group.
use crate::{FileSettings, players};
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use prysm_core::{FramePool, PixelFormat};
use prysm_ports::{Error, FrameSource, Pull, Result};
use std::fmt;
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// Bytes of FFmpeg's stderr kept for error messages.
pub(crate) const STDERR_TAIL: usize = 2048;

/// The FFmpeg invocation, as the shipping application runs it: decode the
/// first video stream once (endlessly with `repeat`: `-stream_loop -1`), cap the rate at `fps`, scale to the frame size
/// mapping HDR to sRGB (FFmpeg 9+), and release frames at their presentation
/// time (`realtime`). Single-threaded video output so frames are not buffered
/// again after pacing. `file:` keeps FFmpeg from reading the path as a URL.
pub(crate) fn ffmpeg(settings: &FileSettings) -> Command {
    let FileSettings {
        path,
        width,
        height,
        fps,
        repeat,
    } = settings;
    let filter = format!(
        "fps='min(source_fps,{fps})',scale={width}:{height}:flags=area:intent=perceptual:\
         out_primaries=bt709:out_transfer=srgb:out_range=full,realtime"
    );
    let mut command = Command::new("ffmpeg");
    command.args(["-nostdin", "-v", "error", "-xerror"]);
    if *repeat {
        command.args(["-stream_loop", "-1"]);
    }
    command
        .arg("-i")
        .arg(format!("file:{path}"))
        .args(["-map", "0:v:0", "-vf", &filter])
        .args(["-fps_mode", "passthrough", "-pix_fmt", "rgb24"])
        .args(["-threads:v", "1"])
        .args(["-f", "rawvideo", "pipe:1"]);
    command
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadlines {
    /// How long the rest of a frame may take once its first byte arrived.
    pub stall: Duration,
    /// How long FFmpeg may take to exit after closing its output.
    pub exit: Duration,
    /// How long `close` waits for the killed child to be reaped. Shorter than
    /// the runtime's session stop deadline so the adapter reports first.
    pub reap: Duration,
}

impl Deadlines {
    pub(crate) const FFMPEG: Self = Self {
        stall: Duration::from_secs(2),
        exit: Duration::from_secs(2),
        reap: Duration::from_secs(3),
    };
}

/// How FFmpeg ended other than by playing the whole file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Ended {
    #[error("FFmpeg exited with {0}")]
    Exited(ExitStatus),
    #[error("FFmpeg closed its output but did not exit within {0:?}")]
    DidNotExit(Duration),
    #[error("FFmpeg stalled mid-frame: {filled} of {expected} bytes within {within:?}")]
    Stalled {
        filled: usize,
        expected: usize,
        within: Duration,
    },
    #[error("killed FFmpeg process {pid} was not reaped within {within:?}")]
    Unreaped { pid: u32, within: Duration },
}

/// How playback failed, with the end of what FFmpeg wrote to stderr.
#[derive(Debug, Clone, thiserror::Error)]
pub(crate) struct DecoderExit {
    pub ended: Ended,
    /// `(filled, expected)` bytes when stdout ended inside a frame.
    pub partial: Option<(usize, usize)>,
    pub stderr: String,
}

impl fmt::Display for DecoderExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((filled, expected)) = self.partial {
            write!(f, "output ended mid-frame ({filled} of {expected} bytes); ")?;
        }
        if self.stderr.is_empty() {
            write!(f, "{}; no stderr output", self.ended)
        } else {
            write!(f, "{}; stderr: {}", self.ended, self.stderr)
        }
    }
}

#[derive(Debug, Default)]
struct Tail {
    bytes: Vec<u8>,
    truncated: bool,
}

impl Tail {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() > STDERR_TAIL {
            self.bytes.drain(..self.bytes.len() - STDERR_TAIL);
            self.truncated = true;
        }
    }

    /// One line: stderr lines joined by ` | `, `…` when the start was dropped.
    fn text(&self) -> String {
        let text = String::from_utf8_lossy(&self.bytes);
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let joined = lines.join(" | ");
        if self.truncated && !joined.is_empty() {
            format!("…{joined}")
        } else {
            joined
        }
    }
}

struct Pipes {
    stdout: ChildStdout,
    /// `None` once stderr reached end of file or failed.
    stderr: Option<ChildStderr>,
    tail: Tail,
}

/// Rounded up so a sub-millisecond remainder does not spin. Longer than
/// `poll` takes (about 65 s) is cut short; every caller polls again until
/// its deadline.
fn poll_timeout(left: Duration) -> PollTimeout {
    let millis = left.as_nanos().div_ceil(1_000_000);
    PollTimeout::from(u16::try_from(millis).unwrap_or(u16::MAX))
}

impl Pipes {
    /// Wait until stdout has data or reached end of file, draining stderr
    /// meanwhile. `false`: `deadline` passed first.
    fn stdout_ready(&mut self, deadline: Instant) -> Result<bool> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (out, err) = {
                let mut stdout = PollFd::new(self.stdout.as_fd(), PollFlags::POLLIN);
                let mut both;
                let fds = if let Some(stderr) = &self.stderr {
                    both = [stdout, PollFd::new(stderr.as_fd(), PollFlags::POLLIN)];
                    both.as_mut_slice()
                } else {
                    std::slice::from_mut(&mut stdout)
                };
                match poll(fds, poll_timeout(left)) {
                    Ok(_) => {}
                    Err(Errno::EINTR) => continue,
                    Err(errno) => {
                        return Err(Error::adapter(
                            "cannot wait for FFmpeg output",
                            io::Error::from(errno),
                        ));
                    }
                }
                (
                    fds[0].any() == Some(true),
                    fds.get(1).and_then(PollFd::any) == Some(true),
                )
            };
            if err {
                self.drain_stderr();
            }
            if out {
                return Ok(true);
            }
            if left.is_zero() {
                return Ok(false);
            }
        }
    }

    /// One read; only called when stderr is readable, so it does not block.
    fn drain_stderr(&mut self) {
        let Some(stderr) = &mut self.stderr else {
            return;
        };
        let mut buffer = [0; 1024];
        match stderr.read(&mut buffer) {
            Ok(0) => self.stderr = None,
            Ok(read) => self.tail.push(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                tracing::debug!(%error, "FFmpeg's stderr is no longer read");
                self.stderr = None;
            }
        }
    }

    /// After stdout ended: collect what FFmpeg still writes to stderr.
    fn finish_stderr(&mut self, deadline: Instant) {
        while let Some(stderr) = &self.stderr {
            let left = deadline.saturating_duration_since(Instant::now());
            let mut fds = [PollFd::new(stderr.as_fd(), PollFlags::POLLIN)];
            match poll(&mut fds, poll_timeout(left)) {
                Ok(_) => {}
                Err(Errno::EINTR) => continue,
                Err(errno) => {
                    tracing::debug!(%errno, "FFmpeg's last stderr is not collected");
                    return;
                }
            }
            if fds[0].any() == Some(true) {
                self.drain_stderr();
            } else if left.is_zero() {
                return;
            }
        }
    }
}

/// The child's exit, reported by a thread blocked in its `wait`, which also
/// reaps it: an event the session thread awaits with a deadline.
struct Exit {
    pid: u32,
    reported: mpsc::Receiver<io::Result<ExitStatus>>,
    status: Option<ExitStatus>,
}

impl Exit {
    fn watch(mut child: Child) -> io::Result<Self> {
        let pid = child.id();
        let (report, reported) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("prysm-ffmpeg-wait".into())
            .spawn(move || {
                let status = child.wait();
                players::forget(pid);
                let _ = report.send(status);
            })?;
        Ok(Self {
            pid,
            reported,
            status: None,
        })
    }

    /// The exit status, waiting until `deadline` for it; `None`: the child
    /// still runs then.
    fn by(&mut self, deadline: Instant) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.reported.recv_timeout(left) {
                Ok(status) => self.status = Some(status?),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other(
                        "the wait for FFmpeg ended without a status",
                    ));
                }
            }
        }
        Ok(self.status)
    }
}

/// `Ok` only once the child has been reaped.
fn kill_and_reap(exit: &mut Exit, within: Duration) -> Result<()> {
    let unconfirmed = |error| Error::adapter("FFmpeg termination is unconfirmed", error);
    if exit.by(Instant::now()).map_err(unconfirmed)?.is_some() {
        return Ok(());
    }
    players::kill_group(exit.pid).map_err(unconfirmed)?;
    match exit.by(Instant::now() + within).map_err(unconfirmed)? {
        Some(_) => Ok(()),
        None => Err(Error::adapter(
            "FFmpeg termination is unconfirmed",
            Ended::Unreaped {
                pid: exit.pid,
                within,
            },
        )),
    }
}

/// A running decoder. Private test seam: `spawn` takes any command that
/// writes raw RGB24 frames of the given size to stdout.
pub(crate) struct FileFrames {
    exit: Exit,
    pipes: Pipes,
    pool: FramePool,
    size: [u32; 2],
    /// The file, for messages.
    name: String,
    deadlines: Deadlines,
    /// The terminal outcome, repeated by later `next_frame` calls: `Ok` when
    /// the file finished playing, else the failure.
    ended: Option<Result<(), Error>>,
    closed: bool,
}

impl FileFrames {
    pub(crate) fn spawn(
        mut command: Command,
        width: u32,
        height: u32,
        name: &str,
        deadlines: Deadlines,
    ) -> Result<Self> {
        PixelFormat::RGB24
            .expected_size(width, height)
            .map_err(|error| Error::adapter("invalid video frame size", error))?;
        let program = command.get_program().to_string_lossy().into_owned();
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|error| {
                let context = if error.kind() == io::ErrorKind::NotFound {
                    format!(
                        "cannot start `{program}`: not found; install FFmpeg 9+ and make it available on PATH"
                    )
                } else {
                    format!("cannot start `{program}`")
                };
                Error::adapter(context, error)
            })?;
        let pid = child.id();
        players::add(pid);
        let pipes = (child.stdout.take(), child.stderr.take());
        let exit = Exit::watch(child).map_err(|error| {
            let _ = players::kill_group(pid);
            Error::adapter("cannot wait for FFmpeg", error)
        });
        let (Some(stdout), Some(stderr)) = pipes else {
            let unreaped = kill_and_reap(&mut exit?, deadlines.reap).err();
            let failure = Error::other(format!("`{program}` output pipes are unavailable"));
            return Err(unreaped.unwrap_or(failure));
        };
        Ok(Self {
            exit: exit?,
            pipes: Pipes {
                stdout,
                stderr: Some(stderr),
                tail: Tail::default(),
            },
            pool: FramePool::default(),
            size: [width, height],
            name: name.to_owned(),
            deadlines,
            ended: None,
            closed: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn pid(&self) -> u32 {
        self.exit.pid
    }

    fn read(&mut self, timeout: Duration) -> Result<Pull> {
        if !self.pipes.stdout_ready(Instant::now() + timeout)? {
            return Ok(Pull::Quiet);
        }
        let [width, height] = self.size;
        let mut writer = self
            .pool
            .acquire(width, height, PixelFormat::RGB24)
            .map_err(|error| Error::adapter("cannot allocate a video frame", error))?;
        let bytes = writer.as_mut_slice();
        let expected = bytes.len();
        let mut filled = 0;
        let stalled_at = Instant::now() + self.deadlines.stall;
        loop {
            match self.pipes.stdout.read(&mut bytes[filled..]) {
                Ok(0) => return self.ended(filled, expected),
                Ok(read) => filled += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(Error::adapter("cannot read FFmpeg output", error)),
            }
            if filled == expected {
                return Ok(Pull::Frame(writer.finish()));
            }
            if !self.pipes.stdout_ready(stalled_at)? {
                let stalled = Ended::Stalled {
                    filled,
                    expected,
                    within: self.deadlines.stall,
                };
                return Err(self.failed(stalled, None));
            }
        }
    }

    /// Stdout reached end of file: reap FFmpeg and say how playback ended.
    fn ended(&mut self, filled: usize, expected: usize) -> Result<Pull> {
        let deadline = Instant::now() + self.deadlines.exit;
        // Until stderr closes: a process FFmpeg left behind may hold it.
        self.pipes.finish_stderr(deadline);
        let status = self
            .exit
            .by(deadline)
            .map_err(|error| Error::adapter("cannot wait for FFmpeg", error))?;
        let partial = (filled > 0).then_some((filled, expected));
        match status {
            Some(status) if status.success() && partial.is_none() => Ok(Pull::Ended),
            Some(status) => Err(self.failed(Ended::Exited(status), partial)),
            None => Err(self.failed(Ended::DidNotExit(self.deadlines.exit), partial)),
        }
    }

    fn failed(&self, ended: Ended, partial: Option<(usize, usize)>) -> Error {
        Error::adapter(
            format!("cannot play video file `{}`", self.name),
            DecoderExit {
                ended,
                partial,
                stderr: self.pipes.tail.text(),
            },
        )
    }
}

impl FrameSource for FileFrames {
    /// [`Pull::Quiet`] when no frame arrived within `timeout`. The end of the file
    /// (`Ended`) and every error are terminal and repeated by later calls.
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        if let Some(ended) = &self.ended {
            return ended.clone().map(|()| Pull::Ended);
        }
        let read = self.read(timeout);
        match &read {
            Ok(Pull::Ended) => self.ended = Some(Ok(())),
            Err(error) => self.ended = Some(Err(error.clone())),
            Ok(_) => {}
        }
        read
    }

    /// Kill FFmpeg (if still running) and certify that it was reaped.
    fn close(mut self: Box<Self>) -> Result<()> {
        self.closed = true;
        kill_and_reap(&mut self.exit, self.deadlines.reap)
    }
}

impl Drop for FileFrames {
    /// Without `close`, FFmpeg is still killed and reaped within the bound.
    fn drop(&mut self) {
        if !self.closed
            && let Err(error) = kill_and_reap(&mut self.exit, self.deadlines.reap)
        {
            tracing::warn!(error = %prysm_ports::chain(&error), "FFmpeg was dropped unconfirmed");
        }
    }
}
