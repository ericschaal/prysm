//! Private, bounded supervisor commands on the required stdin pipe.
use anyhow::{Result, bail};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, poll};
use prysm_webos_protocol::worker::{VideoAvailability, VideoCommand, VideoUpdate};
use std::{
    fs::File,
    io::Read,
    os::fd::AsFd,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct Control {
    input: File,
    availability: VideoAvailability,
}
impl Control {
    pub fn from_stdin() -> Result<Self> {
        let input = File::from(std::io::stdin().as_fd().try_clone_to_owned()?);
        Self::new(input)
    }
    fn new(input: File) -> Result<Self> {
        let flags = OFlag::from_bits_truncate(fcntl(&input, FcntlArg::F_GETFL)?);
        fcntl(&input, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
        Ok(Self {
            input,
            availability: VideoAvailability::Unavailable,
        })
    }
    pub fn video_ready(&mut self, stop: &AtomicBool) -> Result<bool> {
        Ok(!stop.load(Ordering::Relaxed) && self.poll()?.availability.allows_capture())
    }
    pub fn wait_for_video(&mut self, stop: &AtomicBool) -> Result<bool> {
        while !stop.load(Ordering::Relaxed) {
            if self.video_ready(stop)? {
                return Ok(true);
            }
            poll(
                &mut [PollFd::new(self.input.as_fd(), PollFlags::POLLIN)],
                250_u16,
            )?;
        }
        Ok(false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn losing_video_while_idle_closes_the_receiver_connection() {
        use crate::{config::Config, native::Video, session::Session};
        use prysm_webos_protocol::ControlMessage;
        use std::{net::TcpListener, os::fd::OwnedFd, time::Duration};

        let (input, mut updates) = std::io::pipe().unwrap();
        let mut control = Control::new(File::from(OwnedFd::from(input))).unwrap();
        let stop = AtomicBool::new(false);
        updates.write_all(b"1").unwrap();
        assert!(control.wait_for_video(&stop).unwrap());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = Config::new(
            format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap(),
        );
        let peer = std::thread::spawn(move || {
            let mut socket = crate::transport::tests::hello(&listener);
            socket
                .send(tokio_tungstenite::tungstenite::Message::binary(
                    ControlMessage::Idle.encode().unwrap(),
                ))
                .unwrap();
            socket
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            // The receiver never asks for capture. Losing video alone must release the slot.
            assert_eq!(socket.get_mut().read(&mut [0]).unwrap(), 0);
        });
        let mut session = Session::<Video>::connect(&config).unwrap();
        let mut first_poll = true;
        assert!(
            !session
                .wait_for_capture(&config, || {
                    let ready = control.video_ready(&stop)?;
                    if std::mem::take(&mut first_poll) {
                        assert!(ready);
                        updates.write_all(b"0")?;
                    }
                    Ok(ready)
                })
                .unwrap()
        );
        drop(session);
        peer.join().unwrap();
    }

    #[test]
    fn waits_for_video_before_connecting_and_allows_unknown_readiness() {
        for ready in [VideoAvailability::Available, VideoAvailability::Unknown] {
            let (input, mut updates) = std::io::pipe().unwrap();
            let mut control = Control::new(File::from(std::os::fd::OwnedFd::from(input))).unwrap();
            let stop = AtomicBool::new(false);
            updates.write_all(b"0").unwrap();
            std::thread::scope(|scope| {
                let waiting = scope.spawn(|| control.wait_for_video(&stop).unwrap());
                std::thread::sleep(std::time::Duration::from_millis(50));
                assert!(!waiting.is_finished());
                updates
                    .write_all(&[VideoCommand::Availability(ready).into()])
                    .unwrap();
                assert!(waiting.join().unwrap());
            });
            stop.store(true, Ordering::Relaxed);
            assert!(!control.wait_for_video(&stop).unwrap());
        }
    }
}
