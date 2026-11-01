//! Private supervisor pipe, separate from vendor stdout and diagnostic stderr.
use anyhow::{Context, Result, ensure};
use std::{fs::File, io::Write, os::fd::FromRawFd};

#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    VideoResetting,
    VideoReset,
    Connected,
    Capturing,
    Disconnected,
    Failed { message: String },
}

pub struct Events(Option<File>);
impl Events {
    #[cfg(test)]
    pub fn disabled() -> Self {
        Self(None)
    }
    #[cfg(test)]
    pub fn from_file(file: File) -> Self {
        Self(Some(file))
    }
    pub fn from_environment() -> Result<Self> {
        let Some(value) = std::env::var_os("PRYSM_EVENTS_FD") else {
            return Ok(Self(None));
        };
        let descriptor: i32 = value
            .to_str()
            .context("invalid events descriptor")?
            .parse()?;
        ensure!(
            descriptor >= 3,
            "events descriptor must be separate from stdio"
        );
        // SAFETY: validate the inherited descriptor before taking its sole Rust ownership.
        ensure!(
            unsafe { libc::fcntl(descriptor, libc::F_GETFD) } >= 0,
            "invalid events descriptor"
        );
        Ok(Self(Some(unsafe { File::from_raw_fd(descriptor) })))
    }

    pub fn send(&mut self, event: &Event) -> Result<()> {
        if let Some(pipe) = &mut self.0 {
            serde_json::to_writer(&mut *pipe, event)?;
            pipe.write_all(b"\n")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, os::fd::OwnedFd, os::unix::net::UnixStream};
    #[test]
    fn supervisor_events_are_json_on_a_separate_stream() {
        let (writer, mut reader) = UnixStream::pair().unwrap();
        let descriptor: OwnedFd = writer.into();
        let mut events = Events(Some(descriptor.into()));
        events.send(&Event::Connected).unwrap();
        events.send(&Event::Capturing).unwrap();
        drop(events);
        let mut data = String::new();
        reader.read_to_string(&mut data).unwrap();
        assert_eq!(data, "{\"type\":\"connected\"}\n{\"type\":\"capturing\"}\n");
    }
}
