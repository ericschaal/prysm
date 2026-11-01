//! Private, bounded supervisor commands on stdin; standalone capture has no control pipe.
use anyhow::{Result, bail};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use prysm_webos_protocol::worker::{VideoAvailability, VideoCommand, VideoUpdate};
use std::io::{Read, Stdin};

pub struct Control {
    input: Option<Stdin>,
    availability: VideoAvailability,
}
impl Control {
    pub fn from_environment() -> Result<Self> {
        let managed = std::env::var_os("PRYSM_CONTROL").is_some();
        let input = managed.then(std::io::stdin);
        if let Some(input) = &input {
            let flags = OFlag::from_bits_truncate(fcntl(input, FcntlArg::F_GETFL)?);
            fcntl(input, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
        }
        Ok(Self {
            input,
            availability: if managed {
                VideoAvailability::Unavailable
            } else {
                VideoAvailability::Unknown
            },
        })
    }
    pub fn poll(&mut self) -> Result<VideoUpdate> {
        let mut update = VideoUpdate {
            availability: self.availability,
            reset: false,
        };
        if let Some(input) = &mut self.input {
            let mut bytes = [0; 64];
            match input.read(&mut bytes) {
                Ok(0) => bail!("supervisor control pipe closed"),
                Ok(n) => {
                    for byte in &bytes[..n] {
                        update.apply(VideoCommand::try_from(*byte)?);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => (),
                Err(error) => return Err(error.into()),
            }
        }
        self.availability = update.availability;
        Ok(update)
    }
}
