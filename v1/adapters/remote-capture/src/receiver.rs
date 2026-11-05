//! Receiver ports only; protocol, connection lifecycle and capability discovery are deferred.
use super::RemoteCaptureSettings;
use prysm_contract::{
    ControlError, OwnerStopped,
    capture::{
        CaptureCapabilities, CaptureControl, CaptureStart, CaptureStarted, CaptureState,
        CaptureStopped, CapturedFrame, FrameSource,
    },
    identity::SessionId,
    observation::Observation,
};
use std::future::Future;

const UNIMPLEMENTED: ControlError =
    ControlError::Unavailable("skeleton: remote capture receiver is not implemented");

pub struct RemoteCaptureReceiver {
    pub owner: ReceiverOwner,
    pub control: ReceiverControl,
    pub frames: ReceiverFrames,
}

impl RemoteCaptureReceiver {
    /// Capabilities must describe the remote source; transport alone cannot infer them.
    pub fn new(capabilities: CaptureCapabilities) -> Self {
        Self {
            owner: ReceiverOwner(()),
            control: ReceiverControl { capabilities },
            frames: ReceiverFrames(()),
        }
    }
}

pub struct ReceiverOwner(());

impl ReceiverOwner {
    pub async fn run(
        self,
        _shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        Err(UNIMPLEMENTED)
    }
}

#[derive(Clone)]
pub struct ReceiverControl {
    capabilities: CaptureCapabilities,
}

impl CaptureControl for ReceiverControl {
    type Settings = RemoteCaptureSettings;
    fn capabilities(&self) -> CaptureCapabilities {
        self.capabilities.clone()
    }
    fn validate(&self, _settings: &Self::Settings) -> Result<(), ControlError> {
        Err(UNIMPLEMENTED)
    }
    async fn start(
        &self,
        _request: CaptureStart<Self::Settings>,
    ) -> Result<CaptureStarted, ControlError> {
        Err(UNIMPLEMENTED)
    }
    async fn stop(&self, _session: SessionId) -> Result<CaptureStopped, ControlError> {
        Err(UNIMPLEMENTED)
    }
    async fn observe(&self) -> Result<Observation<CaptureState>, ControlError> {
        Err(UNIMPLEMENTED)
    }
}

pub struct ReceiverFrames(());

impl FrameSource for ReceiverFrames {
    async fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ControlError> {
        Err(UNIMPLEMENTED)
    }
}
