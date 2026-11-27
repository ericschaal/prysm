use crate::worker::{Exit, guarded};
use prysm_ports::{Error, Escalation, FrameSource, Result};

pub(super) enum CaptureCleanup {
    NotOpened,
    Close(Box<dyn FrameSource>),
    Unconfirmed(Error),
}

impl CaptureCleanup {
    pub fn after_stream(source: Option<Box<dyn FrameSource>>, exit: &Exit) -> Self {
        match (source, exit) {
            (Some(source), _) => Self::Close(source),
            (None, Exit::Failed(error)) if error.escalation() == Escalation::Restart => {
                Self::Unconfirmed(error.clone())
            }
            (None, _) => Self::NotOpened,
        }
    }

    pub fn close(self) -> Result<()> {
        match self {
            Self::Close(source) => guarded("closing the source", || source.close()),
            Self::NotOpened => Ok(()),
            Self::Unconfirmed(error) => Err(error),
        }
    }
}
