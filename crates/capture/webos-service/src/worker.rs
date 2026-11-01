//! Safe process and bounded stream adapter. No native capture API is used here.
#[allow(unsafe_code)]
mod native;
use crate::{config::Config, supervisor::Event};
use anyhow::Result;
use prysm_webos_protocol::{
    WORKER_CONFIG_ENV,
    worker::{VideoAvailability, VideoUpdate},
};
use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
};

pub struct Worker {
    child: Child,
    control: std::process::ChildStdin,
    video_sent: Option<VideoAvailability>,
    events: File,
    stderr: std::process::ChildStderr,
    pending: Vec<u8>,
    channel_closed: bool,
}
pub struct Update {
    pub events: Vec<std::result::Result<Event, String>>,
    pub log: Option<String>,
    pub exit: Option<ExitStatus>,
}
impl Worker {
    #[cfg(all(test, not(miri)))]
    pub(crate) fn child_id(&self) -> u32 {
        self.child.id()
    }
    pub fn spawn(binary: &Path, config: &Config) -> Result<Self> {
        let mut command = Command::new(binary);
        command
            .env(WORKER_CONFIG_ENV, config.worker_config()?.encode()?)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let (mut child, events) = native::spawn(command)?;
        let control = child.stdin.take().expect("stdin configured as a pipe");
        let stderr = child.stderr.take().expect("stderr configured as a pipe");
        if let Err(error) =
            native::nonblocking(&stderr).and_then(|()| native::nonblocking(&control))
        {
            let _ = child.kill();
            reap(&mut child);
            return Err(error);
        }
        Ok(Self {
            child,
            control,
            video_sent: None,
            events,
            stderr,
            pending: Vec::new(),
            channel_closed: false,
        })
    }
    pub fn update_video(&mut self, update: VideoUpdate) -> Result<bool> {
        if self.video_sent == Some(update.availability) && !update.reset {
            return Ok(true);
        }
        match self.control.write(&[update.command().into()]) {
            Ok(1) => self.video_sent = Some(update.availability),
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error.into()),
            Ok(_) => anyhow::bail!("worker control pipe closed"),
        }
        Ok(true)
    }
    pub fn terminate(&mut self) -> Result<()> {
        use nix::{
            sys::signal::{Signal, kill},
            unistd::Pid,
        };
        match kill(Pid::from_raw(self.child.id().try_into()?), Signal::SIGTERM) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    pub fn kill(&mut self) -> Result<()> {
        self.child.kill().map_err(Into::into)
    }
    pub fn poll(&mut self) -> Result<Update> {
        let mut update = Update {
            events: Vec::new(),
            log: None,
            exit: None,
        };
        let mut buffer = [0_u8; 8192];
        let was_closed = self.channel_closed;
        let mut eof = false;
        let mut drained = self.channel_closed;
        for _ in 0..8 {
            if self.channel_closed {
                break;
            }
            match self.events.read(&mut buffer) {
                Ok(0) => {
                    self.channel_closed = true;
                    eof = true;
                    drained = true;
                    break;
                }
                Ok(count) => {
                    self.pending.extend_from_slice(&buffer[..count]);
                    decode(&mut self.pending, &mut update.events);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    drained = true;
                    break;
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => (),
                Err(error) => return Err(error.into()),
            }
        }
        match self.stderr.read(&mut buffer) {
            Ok(count) if count > 0 => {
                update.log = Some(
                    String::from_utf8_lossy(&buffer[count.saturating_sub(2000)..count])
                        .into_owned(),
                );
            }
            Ok(_) => (),
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
            Err(error) => return Err(error.into()),
        }
        if drained {
            update.exit = self.child.try_wait()?;
        }
        // Let an orderly exit close its pipe before waitpid observes it.
        if (eof && !self.pending.is_empty()) || (was_closed && update.exit.is_none()) {
            update.events.push(Err(if self.pending.is_empty() {
                "Capturer status channel closed".into()
            } else {
                "Incomplete capturer status".into()
            }));
            self.pending.clear();
        }
        Ok(update)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // Never leave an owned child running after an application error.
        let _ = self.child.kill();
        reap(&mut self.child);
    }
}

pub(crate) fn reap(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    let Ok(pid) = i32::try_from(child.id()) else {
        return;
    };
    // Abnormal teardown must not block the GLib thread on a kernel wait.
    if let Err(error) = std::thread::Builder::new()
        .name("child-reaper".into())
        .spawn(move || {
            while let Err(nix::errno::Errno::EINTR) =
                nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None)
            {}
        })
    {
        eprintln!("cannot start reaper for killed child {pid}: {error}");
    }
}
fn decode(pending: &mut Vec<u8>, events: &mut Vec<std::result::Result<Event, String>>) {
    while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
        let event = if end > 4096 {
            Err("Oversized capturer status".into())
        } else {
            serde_json::from_slice(&pending[..end]).map_err(|_| "Invalid capturer status".into())
        };
        pending.drain(..=end);
        let invalid = event.is_err();
        events.push(event);
        if invalid {
            pending.clear();
            return;
        }
    }
    if pending.len() > 4096 {
        pending.clear();
        events.push(Err("Oversized capturer status".into()));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fragmented_events_and_both_complete_and_partial_oversized_lines_are_bounded() {
        let mut pending = b"{\"type\":\"con".to_vec();
        let mut events = Vec::new();
        super::decode(&mut pending, &mut events);
        assert_eq!(events, []);
        pending.extend_from_slice(b"nected\"}\n{\"type\":\"capturing\"}\n");
        super::decode(&mut pending, &mut events);
        assert_eq!(events.len(), 2);
        for suffix in [b"".as_slice(), b"\n".as_slice()] {
            let mut pending = vec![b'x'; 4097];
            pending.extend_from_slice(suffix);
            let mut events = Vec::new();
            super::decode(&mut pending, &mut events);
            assert!(events[0].is_err());
            assert_eq!(pending, [] as [u8; 0]);
        }
    }
}

#[cfg(all(test, not(miri)))]
mod process_tests {
    use super::*;
    use crate::application::tests::{sandbox, script};
    use std::time::{Duration, Instant};
    fn config() -> Config {
        Config {
            receiver: Some("127.0.0.1:42".parse().unwrap()),
            metrics: true,
            ..Config::default()
        }
    }
    #[test]
    fn control_pipe_deduplicates_readiness_and_preserves_explicit_reset() {
        let dir = sandbox();
        let path = dir.path().join("worker");
        script(
            &path,
            "printf '%s\\n' \"$#\" \"$PRYSM_WORKER_CONFIG\" > \"$0.startup\"; dd bs=1 count=6 >&2 2>/dev/null",
        );
        let mut worker = Worker::spawn(&path, &config()).unwrap();
        for (availability, reset) in [
            (VideoAvailability::Unavailable, false),
            (VideoAvailability::Unavailable, false),
            (VideoAvailability::Available, false),
            (VideoAvailability::Available, true),
            (VideoAvailability::Unknown, false),
            (VideoAvailability::Unknown, true),
            (VideoAvailability::Unknown, false),
            (VideoAvailability::Unavailable, true),
        ] {
            assert!(
                worker
                    .update_video(VideoUpdate {
                        availability,
                        reset
                    })
                    .unwrap()
            );
        }
        let mut log = String::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let update = worker.poll().unwrap();
            if let Some(chunk) = update.log {
                log.push_str(&chunk);
            }
            if update.exit.is_some() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(log, "01r?r0");
        let startup = std::fs::read_to_string(dir.path().join("worker.startup")).unwrap();
        let mut lines = startup.lines();
        assert_eq!(lines.next(), Some("0"));
        assert_eq!(
            prysm_webos_protocol::WorkerConfig::decode(lines.next().unwrap()).unwrap(),
            config().worker_config().unwrap()
        );
    }
    #[test]
    fn inherited_fd_fragmentation_stderr_pressure_and_exit_are_observed() {
        let dir = sandbox();
        let path = dir.path().join("worker");
        script(
            &path,
            "printf '{\"type\":\"con' >&\"$PRYSM_EVENTS_FD\"; sleep 0.02; printf 'nected\"}\\n' >&\"$PRYSM_EVENTS_FD\"; dd if=/dev/zero bs=8192 count=32 >&2 2>/dev/null; printf '%s\\n' '{\"type\":\"disconnected\"}' >&\"$PRYSM_EVENTS_FD\"",
        );
        let mut worker = Worker::spawn(&path, &config()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut connected = false;
        let mut disconnected = false;
        let mut logged = false;
        loop {
            let update = worker.poll().unwrap();
            for event in update.events {
                match event {
                    Ok(Event::Connected) => connected = true,
                    Ok(Event::Disconnected) => disconnected = true,
                    _ => (),
                }
            }
            logged |= update.log.is_some();
            if update.exit.is_some() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(connected && disconnected && logged);
        assert!(Worker::spawn(&dir.path().join("missing"), &config()).is_err());
    }
    #[test]
    fn event_eof_with_live_child_and_incomplete_last_line_fail_closed() {
        for body in [
            "eval \"exec $PRYSM_EVENTS_FD>&-\"; exec sleep 30",
            "printf '{' >&\"$PRYSM_EVENTS_FD\"; eval \"exec $PRYSM_EVENTS_FD>&-\"; exec sleep 30",
        ] {
            let dir = sandbox();
            let path = dir.path().join("worker");
            script(&path, body);
            let mut worker = Worker::spawn(&path, &config()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let update = worker.poll().unwrap();
                if update.events.iter().any(Result::is_err) {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            let before = Instant::now();
            drop(worker);
            assert!(before.elapsed() < Duration::from_secs(1));
        }
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn worker_dies_when_supervisor_exits_without_dropping_it() {
        if let Ok(path) = std::env::var("PRYSM_TEST_PARENT_DEATH") {
            let mut command = Command::new("sh");
            command.args(["-c", "exec sleep 30"]);
            let (child, _events) = native::spawn(command).unwrap();
            std::fs::write(path, child.id().to_string()).unwrap();
            std::process::exit(0);
        }
        let dir = sandbox();
        let path = dir.path().join("pid");
        assert!(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "worker::process_tests::worker_dies_when_supervisor_exits_without_dropping_it"
                ])
                .env("PRYSM_TEST_PARENT_DEATH", &path)
                .status()
                .unwrap()
                .success()
        );
        let pid = std::fs::read_to_string(path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            if stat.is_err()
                || stat.as_ref().is_ok_and(|stat| {
                    stat.split(") ")
                        .nth(1)
                        .is_some_and(|tail| tail.starts_with('Z'))
                })
            {
                break;
            }
            assert!(Instant::now() < deadline, "worker outlived its parent");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
