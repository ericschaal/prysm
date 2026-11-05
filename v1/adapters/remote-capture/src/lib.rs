//! Remote capture transport endpoints, independent of the source platform.
use prysm_contract::{
    ControlError, OwnerStopped,
    capture::{CaptureControl, FrameSource},
};
use std::{future::Future, net::SocketAddr};

/// Remote capture requests drive the local source; its frames go to this peer.
pub struct RemoteCaptureSender<C, F> {
    pub capture: C,
    pub frames: F,
    pub peer: SocketAddr,
}

impl<C: CaptureControl, F: FrameSource> RemoteCaptureSender<C, F> {
    pub fn connect(capture: C, frames: F, peer: SocketAddr) -> Self {
        Self {
            capture,
            frames,
            peer,
        }
    }

    pub async fn run(
        self,
        _shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        Err(ControlError::Unavailable(
            "skeleton: remote capture requests and frame transport are not implemented",
        ))
    }
}

/// Receiver connection settings. Resolution/rate belong to the producing source.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "wire", serde(deny_unknown_fields))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct RemoteCaptureSettings {
    #[cfg_attr(feature = "schema", schema(value_type = String))]
    pub peer: SocketAddr,
}

pub mod receiver;
pub use receiver::RemoteCaptureReceiver;

#[cfg(feature = "schema")]
pub mod schema;
