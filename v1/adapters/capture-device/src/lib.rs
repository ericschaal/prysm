//! Owned video device capture, independent of processing and output.
mod backend;
mod mailbox;
mod owner;
mod session;

use mailbox::Mailbox;
use prysm_contract::{
    ControlError, OwnerStopped,
    capture::{
        CaptureCapabilities, CaptureControl, CaptureStart, CaptureStarted, CaptureState,
        CaptureStopped, CapturedFrame, FrameSource,
    },
    identity::{InstanceId, SessionId},
    observation::Observation,
};
use std::{future::Future, sync::Arc};
use tokio::sync::{mpsc, oneshot};

const CAPACITY: usize = 16;
const CLOSED: ControlError = ControlError::Unavailable("video device capture owner closed");
type Reply<T> = oneshot::Sender<Result<T, ControlError>>;
enum Command {
    Start(CaptureStart<DeviceCaptureSettings>, Reply<CaptureStarted>),
    Stop(SessionId, Reply<CaptureStopped>),
}

pub struct DeviceCapture {
    pub owner: DeviceOwner,
    pub control: DeviceHandle,
    pub frames: DeviceFrames,
}
impl DeviceCapture {
    /// Inert wiring, using the same host identity as application and processing.
    pub fn new(instance: InstanceId) -> Self {
        let mailbox = Arc::new(Mailbox::new(instance));
        let (commands, receiver) = mpsc::channel(CAPACITY);
        Self {
            owner: DeviceOwner {
                instance,
                commands: receiver,
                mailbox: mailbox.clone(),
            },
            control: DeviceHandle {
                commands,
                mailbox: mailbox.clone(),
            },
            frames: DeviceFrames(mailbox),
        }
    }
}
pub struct DeviceOwner {
    instance: InstanceId,
    commands: mpsc::Receiver<Command>,
    mailbox: Arc<Mailbox>,
}
impl DeviceOwner {
    /// Retain on host Tokio through shutdown: Stop success requires native release.
    pub async fn run(
        self,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        owner::run(self, shutdown, backend::open).await
    }
}
impl Drop for DeviceOwner {
    fn drop(&mut self) {
        self.mailbox.close();
    }
}
#[derive(Clone)]
pub struct DeviceHandle {
    commands: mpsc::Sender<Command>,
    mailbox: Arc<Mailbox>,
}
impl DeviceHandle {
    async fn request<T>(
        &self,
        command: impl FnOnce(Reply<T>) -> Command,
    ) -> Result<T, ControlError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(command(reply))
            .await
            .map_err(|_| CLOSED)?;
        response.await.map_err(|_| CLOSED)?
    }
}
impl CaptureControl for DeviceHandle {
    type Settings = DeviceCaptureSettings;
    fn capabilities(&self) -> CaptureCapabilities {
        CaptureCapabilities {
            supports_patterns: false,
        }
    }
    fn validate(&self, settings: &Self::Settings) -> Result<(), ControlError> {
        settings.validate()
    }
    async fn start(
        &self,
        request: CaptureStart<Self::Settings>,
    ) -> Result<CaptureStarted, ControlError> {
        self.request(|reply| Command::Start(request, reply)).await
    }
    async fn stop(&self, session: SessionId) -> Result<CaptureStopped, ControlError> {
        self.request(|reply| Command::Stop(session, reply)).await
    }
    async fn observe(&self) -> Result<Observation<CaptureState>, ControlError> {
        self.mailbox.observe()
    }
}
pub struct DeviceFrames(Arc<Mailbox>);
impl FrameSource for DeviceFrames {
    async fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ControlError> {
        Ok(self.0.next().await)
    }
}
use prysm_core::CaptureResolution;
#[cfg(feature = "schema")]
use prysm_protocol::schema::resolution as resolution_schema;
#[cfg(feature = "wire")]
use prysm_protocol::wire::resolution as resolution_wire;
use std::{num::NonZeroU32, path::PathBuf};

/// These choices belong to the device adapter, never the shared application.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "wire", serde(tag = "source", rename_all = "snake_case"))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum DeviceCaptureSettings {
    Device {
        #[cfg_attr(feature = "schema", schema(value_type = String))]
        path: PathBuf,
        #[cfg_attr(feature = "wire", serde(with = "resolution_wire"))]
        #[cfg_attr(feature = "schema", schema(schema_with = resolution_schema))]
        resolution: CaptureResolution,
        #[cfg_attr(feature = "schema", schema(value_type = u32, minimum = 1, maximum = 4294967295_u64))]
        rate: NonZeroU32,
    },
    File {
        #[cfg_attr(feature = "schema", schema(value_type = String))]
        path: PathBuf,
        #[cfg_attr(feature = "wire", serde(with = "resolution_wire"))]
        #[cfg_attr(feature = "schema", schema(schema_with = resolution_schema))]
        resolution: CaptureResolution,
        #[cfg_attr(feature = "schema", schema(value_type = u32, minimum = 1, maximum = 4294967295_u64))]
        rate: NonZeroU32,
    },
}

impl DeviceCaptureSettings {
    fn validate(&self) -> Result<(), ControlError> {
        match self {
            Self::File { .. } => Err(ControlError::Unavailable("file capture is not implemented")),
            Self::Device { path, rate, .. } => {
                if path.as_os_str().is_empty()
                    || path.to_str().is_none_or(|path| path.contains('\0'))
                {
                    return Err(ControlError::Invalid {
                        field: "path",
                        reason: "a nonempty UTF-8 device identifier is required".into(),
                    });
                }
                if cfg!(target_os = "macos") && rate.get() > i32::MAX as u32 {
                    return Err(ControlError::Invalid {
                        field: "rate",
                        reason: "rate exceeds native timescale".into(),
                    });
                }
                if !cfg!(any(target_os = "linux", target_os = "macos")) {
                    return Err(ControlError::Unavailable(
                        "video device capture is unsupported on this platform",
                    ));
                }
                Ok(())
            }
        }
    }
}

#[cfg(feature = "schema")]
pub mod schema;

#[cfg(test)]
mod tests;
