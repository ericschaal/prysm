//! One running FFmpeg child. The session thread polls the child's stdout and
//! stderr with a deadline (no helper thread): stdout carries whole RGB24
//! frames, stderr is drained into a bounded tail that failures carry.
use crate::FileSettings;
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use prysm_core::{Frame, FramePool, PixelFormat};
use prysm_ports::{Error, FrameSource, Result};
use std::fmt;
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Bytes of FFmpeg's stderr kept for error messages.
pub(crate) const STDERR_TAIL: usize = 2048;

/// The FFmpeg invocation, as the shipping application runs it: decode the
/// first video stream once, cap the rate at `fps`, scale to the frame size
/// mapping HDR to sRGB (FFmpeg 9+), and release frames at their presentation
/// time (`realtime`). Single-threaded video output so frames are not buffered
/// again after pacing. `file:` keeps FFmpeg from reading the path as a URL.
pub(crate) fn ffmpeg(settings: &FileSettings) -> Command {
    let FileSettings {
        path,
        width,
        height,
        fps,
    } = settings;
    let filter = format!(
        "fps='min(source_fps,{fps})',scale={width}:{height}:flags=area:intent=perceptual:\
         out_primaries=bt709:out_transfer=srgb:out_range=full,realtime"
    );
    let mut command = Command::new("ffmpeg");
    command
        .args(["-nostdin", "-v", "error", "-xerror", "-i"])
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

/// How FFmpeg ended, with the end of what it wrote to stderr.
#[derive(Debug, Clone, thiserror::Error)]
pub(crate) struct DecoderExit {
    pub outcome: String,
    pub stderr: String,
}

impl fmt::Display for DecoderExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.stderr.is_empty() {
            write!(f, "{}; no stderr output", self.outcome)
        } else {
            write!(f, "{}; stderr: {}", self.outcome, self.stderr)
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

fn readable(fd: &PollFd<'_>) -> bool {
    fd.revents().is_some_and(|events| !events.is_empty())
}

/// Rounded up so a sub-millisecond remainder does not spin.
fn poll_timeout(left: Duration) -> PollTimeout {
    let millis = left.as_nanos().div_ceil(1_000_000);
    PollTimeout::from(u16::try_from(millis).unwrap_or(u16::MAX))
}

impl Pipes {
    /// Wait until stdout has data or reached end of file, draining stderr
    /// meanwhile. `false`: `deadline` passed first.
    fn stdout_ready(&mut self, deadline: Instant) -> io::Result<bool> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (out, err) = {
                let mut fds = vec![PollFd::new(self.stdout.as_fd(), PollFlags::POLLIN)];
                if let Some(stderr) = &self.stderr {
                    fds.push(PollFd::new(stderr.as_fd(), PollFlags::POLLIN));
                }
                match poll(&mut fds, poll_timeout(left)) {
                    Ok(_) => {}
                    Err(Errno::EINTR) => continue,
                    Err(errno) => return Err(errno.into()),
                }
                (readable(&fds[0]), fds.get(1).is_some_and(readable))
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
            Err(_) => self.stderr = None,
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
                Err(_) => return,
            }
            let ready = readable(&fds[0]);
            if ready {
                self.drain_stderr();
            } else if left.is_zero() {
                return;
            }
        }
    }
}

enum Filled {
    Frame(Frame),
    /// Stdout ended after `filled` bytes of a frame.
    Ended {
        filled: usize,
        expected: usize,
    },
    Stalled {
        filled: usize,
        expected: usize,
    },
}

/// Fill one pooled frame. The first byte is already readable.
fn read_frame(
    pipes: &mut Pipes,
    pool: &mut FramePool,
    [width, height]: [u32; 2],
    stall: Duration,
) -> Result<Filled> {
    let mut writer = pool
        .acquire(width, height, PixelFormat::RGB24)
        .map_err(|error| Error::adapter("cannot allocate a video frame", error))?;
    let expected = writer.as_mut_slice().len();
    let stalled_at = Instant::now() + stall;
    let mut filled = 0;
    loop {
        match pipes.stdout.read(&mut writer.as_mut_slice()[filled..]) {
            Ok(0) => return Ok(Filled::Ended { filled, expected }),
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(Error::adapter("cannot read FFmpeg output", error)),
        }
        if filled == expected {
            return Ok(Filled::Frame(writer.finish()));
        }
        if !pipes
            .stdout_ready(stalled_at)
            .map_err(|error| Error::adapter("cannot wait for FFmpeg output", error))?
        {
            return Ok(Filled::Stalled { filled, expected });
        }
    }
}

/// Poll the child until it exits or `deadline` passes.
fn wait_until(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(2));
    }
}

/// `Ok` only once the child has been reaped.
fn kill_and_reap(child: &mut Child, within: Duration) -> Result<()> {
    let unconfirmed = |error| Error::adapter("FFmpeg termination is unconfirmed", error);
    if child.try_wait().map_err(unconfirmed)?.is_some() {
        return Ok(());
    }
    child.kill().map_err(unconfirmed)?;
    match wait_until(child, Instant::now() + within).map_err(unconfirmed)? {
        Some(_) => Ok(()),
        None => Err(Error::other(format!(
            "FFmpeg termination is unconfirmed: killed process {} was not reaped within {within:?}",
            child.id()
        ))),
    }
}

/// A running decoder. Private test seam: `spawn` takes any command that
/// writes raw RGB24 frames of the given size to stdout.
pub(crate) struct FileFrames {
    child: Child,
    pipes: Pipes,
    pool: FramePool,
    size: [u32; 2],
    /// The file, for messages.
    name: String,
    deadlines: Deadlines,
    /// The terminal error, repeated by later `next_frame` calls.
    ended: Option<Error>,
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
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let unreaped = kill_and_reap(&mut child, deadlines.reap).err();
            let failure = Error::other(format!("`{program}` output pipes are unavailable"));
            return Err(unreaped.unwrap_or(failure));
        };
        Ok(Self {
            child,
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
        self.child.id()
    }

    fn read(&mut self, timeout: Duration) -> Result<Option<Frame>> {
        if !self
            .pipes
            .stdout_ready(Instant::now() + timeout)
            .map_err(|error| Error::adapter("cannot wait for FFmpeg output", error))?
        {
            return Ok(None);
        }
        match read_frame(
            &mut self.pipes,
            &mut self.pool,
            self.size,
            self.deadlines.stall,
        )? {
            Filled::Frame(frame) => Ok(Some(frame)),
            Filled::Ended { filled, expected } => Err(self.ended(filled, expected)),
            Filled::Stalled { filled, expected } => Err(Error::other(format!(
                "FFmpeg stalled mid-frame playing `{}`: {filled} of {expected} bytes within {:?}",
                self.name, self.deadlines.stall
            ))),
        }
    }

    /// Stdout reached end of file: reap FFmpeg and say how playback ended.
    fn ended(&mut self, filled: usize, expected: usize) -> Error {
        let deadline = Instant::now() + self.deadlines.exit;
        self.pipes.finish_stderr(deadline);
        let status = wait_until(&mut self.child, deadline);
        let failed = |outcome: String, stderr: String| {
            Error::adapter(
                format!("cannot play video file `{}`", self.name),
                DecoderExit { outcome, stderr },
            )
        };
        let stderr = self.pipes.tail.text();
        let partial = if filled > 0 {
            format!("output ended mid-frame ({filled} of {expected} bytes); ")
        } else {
            String::new()
        };
        match status {
            Ok(Some(status)) if status.success() && filled == 0 => {
                Error::other(format!("video file `{}` finished playing", self.name))
            }
            Ok(Some(status)) => failed(format!("{partial}FFmpeg exited with {status}"), stderr),
            Ok(None) => failed(
                format!(
                    "{partial}FFmpeg closed its output but did not exit within {:?}",
                    self.deadlines.exit
                ),
                stderr,
            ),
            Err(error) => Error::adapter("cannot wait for FFmpeg", error),
        }
    }
}

impl FrameSource for FileFrames {
    /// `Ok(None)` when no frame arrived within `timeout`. Every error is
    /// terminal and repeated by later calls; the end of the file is the
    /// error "finished playing".
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Frame>> {
        if let Some(ended) = &self.ended {
            return Err(ended.clone());
        }
        let read = self.read(timeout);
        if let Err(error) = &read {
            self.ended = Some(error.clone());
        }
        read
    }

    /// Kill FFmpeg (if still running) and certify that it was reaped.
    fn close(mut self: Box<Self>) -> Result<()> {
        self.closed = true;
        kill_and_reap(&mut self.child, self.deadlines.reap)
    }
}

impl Drop for FileFrames {
    /// Without `close`, FFmpeg is still killed and reaped within the bound.
    fn drop(&mut self) {
        if !self.closed {
            let _ = kill_and_reap(&mut self.child, self.deadlines.reap);
        }
    }
}
