//! Private supervisor/worker IPC, separate from the receiver's video protocol.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerEvent {
    EncoderBusy,
    EncoderIdle,
    VideoResetting,
    VideoReset,
    Connected,
    Capturing,
    Disconnected,
    Failed { message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoAvailability {
    Unknown,
    Unavailable,
    Available,
}

impl VideoAvailability {
    /// Unknown readiness permits the worker's bounded native retry fallback.
    #[must_use]
    pub fn allows_capture(self) -> bool {
        self != Self::Unavailable
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCommand {
    Availability(VideoAvailability),
    /// Reset the video client and permit capture, including unknown-readiness fallback.
    Reset,
}

impl From<VideoCommand> for u8 {
    fn from(command: VideoCommand) -> Self {
        match command {
            VideoCommand::Availability(VideoAvailability::Unavailable) => b'0',
            VideoCommand::Availability(VideoAvailability::Available) => b'1',
            VideoCommand::Availability(VideoAvailability::Unknown) => b'?',
            VideoCommand::Reset => b'r',
        }
    }
}

impl TryFrom<u8> for VideoCommand {
    type Error = anyhow::Error;

    fn try_from(byte: u8) -> Result<Self> {
        match byte {
            b'0' => Ok(Self::Availability(VideoAvailability::Unavailable)),
            b'1' => Ok(Self::Availability(VideoAvailability::Available)),
            b'?' => Ok(Self::Availability(VideoAvailability::Unknown)),
            b'r' => Ok(Self::Reset),
            _ => bail!("invalid supervisor command"),
        }
    }
}

/// Latest readiness plus any reset retained across rapid source transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoUpdate {
    pub availability: VideoAvailability,
    pub reset: bool,
}

impl VideoUpdate {
    pub fn observe(&mut self, availability: VideoAvailability) {
        self.reset |= self.availability.allows_capture() && !availability.allows_capture();
        self.availability = availability;
    }

    pub fn apply(&mut self, command: VideoCommand) {
        match command {
            VideoCommand::Availability(availability) => self.observe(availability),
            VideoCommand::Reset => {
                self.availability = VideoAvailability::Available;
                self.reset = true;
            }
        }
    }

    #[must_use]
    pub fn command(self) -> VideoCommand {
        if self.reset && self.availability.allows_capture() {
            VideoCommand::Reset
        } else {
            VideoCommand::Availability(self.availability)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_bytes_round_trip_and_reject_unknown_commands() {
        for (byte, command) in [
            (
                b'0',
                VideoCommand::Availability(VideoAvailability::Unavailable),
            ),
            (
                b'1',
                VideoCommand::Availability(VideoAvailability::Available),
            ),
            (b'?', VideoCommand::Availability(VideoAvailability::Unknown)),
            (b'r', VideoCommand::Reset),
        ] {
            assert_eq!(u8::from(command), byte);
            assert_eq!(VideoCommand::try_from(byte).unwrap(), command);
        }
        assert!(VideoCommand::try_from(b'x').is_err());
    }

    #[test]
    fn loss_and_return_remain_latched_even_when_readiness_becomes_unknown() {
        let mut update = VideoUpdate {
            availability: VideoAvailability::Available,
            reset: false,
        };
        for byte in b"01?" {
            update.apply(VideoCommand::try_from(*byte).unwrap());
        }
        assert!(update.reset);
        assert_eq!(update.command(), VideoCommand::Reset);
        update.apply(update.command());
        assert!(update.availability.allows_capture());
        update.reset = false;
        update.apply(VideoCommand::Availability(VideoAvailability::Unavailable));
        assert!(update.reset);
        assert_eq!(
            update.command(),
            VideoCommand::Availability(VideoAvailability::Unavailable)
        );
    }

    #[test]
    fn worker_events_keep_the_existing_json_contract() {
        for event in [
            WorkerEvent::EncoderBusy,
            WorkerEvent::EncoderIdle,
            WorkerEvent::VideoResetting,
            WorkerEvent::VideoReset,
            WorkerEvent::Connected,
            WorkerEvent::Capturing,
            WorkerEvent::Disconnected,
            WorkerEvent::Failed {
                message: "failed".into(),
            },
        ] {
            let json = serde_json::to_vec(&event).unwrap();
            assert_eq!(serde_json::from_slice::<WorkerEvent>(&json).unwrap(), event);
        }
        assert_eq!(
            serde_json::to_string(&WorkerEvent::VideoReset).unwrap(),
            r#"{"type":"video_reset"}"#
        );
    }
}
