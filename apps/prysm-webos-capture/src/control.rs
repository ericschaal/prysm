//! Private, bounded supervisor commands on the required stdin pipe.
use anyhow::{Result, bail};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use prysm_webos_protocol::worker::{VideoAvailability, VideoCommand, VideoUpdate};
use std::io::{Read, Stdin};

pub struct Control {
    input: Stdin,
    availability: VideoAvailability,
}
impl Control {
    pub fn from_stdin() -> Result<Self> {
        let input = std::io::stdin();
        let flags = OFlag::from_bits_truncate(fcntl(&input, FcntlArg::F_GETFL)?);
        fcntl(&input, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
        Ok(Self {
            input,
            availability: VideoAvailability::Unavailable,
        })
    }
    pub fn poll(&mut self) -> Result<VideoUpdate> {
        let mut update = VideoUpdate {
            availability: self.availability,
            reset: false,
        };
        let mut bytes = [0; 64];
        match self.input.read(&mut bytes) {
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
        self.availability = update.availability;
        Ok(update)
    }
}
