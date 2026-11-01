//! Private, bounded supervisor commands on stdin; standalone capture has no control pipe.
use anyhow::{Result, bail};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use std::io::{Read, Stdin};

pub struct Control {
    input: Option<Stdin>,
    available: bool,
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
            available: !managed,
        })
    }
    pub fn poll(&mut self) -> Result<(bool, bool)> {
        let mut reset = false;
        if let Some(input) = &mut self.input {
            let mut bytes = [0; 64];
            match input.read(&mut bytes) {
                Ok(0) => bail!("supervisor control pipe closed"),
                Ok(n) => reset = apply(&mut self.available, &bytes[..n])?,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok((self.available, reset))
    }
}
fn apply(available: &mut bool, bytes: &[u8]) -> Result<bool> {
    let mut reset = false;
    for byte in bytes {
        let next = match byte {
            b'0' => false,
            b'1' | b'?' | b'r' => true,
            _ => bail!("invalid supervisor command"),
        };
        reset |= (*available && !next) || *byte == b'r';
        *available = next;
    }
    Ok(reset)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rapid_loss_and_return_still_reset_the_old_video_client() {
        let mut available = true;
        assert!(apply(&mut available, b"01").unwrap());
        assert!(available);
        assert!(!apply(&mut available, b"11").unwrap());
        assert!(apply(&mut available, b"0").unwrap());
        assert!(!available);
        assert!(!apply(&mut available, b"?").unwrap());
        assert!(available);
        assert!(apply(&mut available, b"r").unwrap());
        assert!(available);
        assert!(apply(&mut available, b"x").is_err());
    }
}
