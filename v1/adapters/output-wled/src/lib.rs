//! One owned WLED writer: session fences, latest colors, mode changes and refresh.
mod mailbox;
mod owner;
mod transport;

use mailbox::Mailbox;
use prysm_contract::{
    ControlError, OwnerStopped,
    identity::{InstanceId, SessionId, StreamEpoch},
    observation::Observation,
    output::{
        ColorFrame, ColorSink, IdleOutputMode, OfferResult, OutputApplied, OutputCapabilities,
        OutputControl, OutputState,
    },
};
use prysm_core::EdgeColors;
use std::{future::Future, sync::Arc};
use tokio::sync::{mpsc, oneshot};

const CAPACITY: usize = 16;
const CLOSED: ControlError = ControlError::Unavailable("WLED output owner closed");
type Reply = oneshot::Sender<Result<OutputApplied, ControlError>>;
enum Command {
    Configure(WledSettings, Reply),
    Admit(StreamEpoch, Reply),
    Hold(StreamEpoch, EdgeColors, Reply),
    Finish(SessionId, IdleOutputMode, Reply),
}

pub struct WledOutput {
    pub owner: WledOwner,
    pub control: WledHandle,
    pub colors: WledColors,
    pub shutdown: WledShutdown,
}
impl WledOutput {
    /// Inert wiring. Use the same host instance as the application and flow.
    pub fn new(instance: InstanceId) -> Self {
        let mailbox = Arc::new(Mailbox::new(instance));
        let (commands, receiver) = mpsc::channel(CAPACITY);
        Self {
            owner: WledOwner {
                commands: receiver,
                mailbox: mailbox.clone(),
            },
            control: WledHandle {
                commands,
                mailbox: mailbox.clone(),
            },
            colors: WledColors(mailbox.clone()),
            shutdown: WledShutdown(mailbox),
        }
    }
}

/// Irreversibly retires this output. Requesting shutdown is not proof of cleanup;
/// retain and await the owner, including any write already in flight.
#[derive(Clone)]
pub struct WledShutdown(Arc<Mailbox>);
impl WledShutdown {
    pub fn request(&self) {
        self.0.request_shutdown();
    }
}
pub struct WledOwner {
    commands: mpsc::Receiver<Command>,
    mailbox: Arc<Mailbox>,
}
impl WledOwner {
    /// Drive on host Tokio. Shutdown settles the current write, then requests Off.
    /// Retain this future through a stalled write; dropping it cannot certify cleanup.
    pub async fn run(
        self,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        let network = transport::Network::new()?;
        owner::run(self, network, shutdown).await
    }
}
impl Drop for WledOwner {
    fn drop(&mut self) {
        self.mailbox.close();
    }
}
#[derive(Clone)]
pub struct WledHandle {
    commands: mpsc::Sender<Command>,
    mailbox: Arc<Mailbox>,
}
impl WledHandle {
    async fn request(
        &self,
        command: impl FnOnce(Reply) -> Command,
    ) -> Result<OutputApplied, ControlError> {
        self.mailbox.authorize()?;
        let (reply, response) = oneshot::channel();
        self.commands
            .send(command(reply))
            .await
            .map_err(|_| CLOSED)?;
        response.await.map_err(|_| CLOSED)?
    }
}
impl OutputControl for WledHandle {
    type Settings = WledSettings;
    fn capabilities(&self) -> OutputCapabilities {
        OutputCapabilities { native_mode: true }
    }
    fn validate(&self, settings: &WledSettings) -> Result<(), ControlError> {
        settings.validate()
    }
    async fn configure(&self, settings: WledSettings) -> Result<OutputApplied, ControlError> {
        self.request(|reply| Command::Configure(settings, reply))
            .await
    }
    async fn admit(&self, epoch: StreamEpoch) -> Result<OutputApplied, ControlError> {
        self.request(|reply| Command::Admit(epoch, reply)).await
    }
    async fn hold(
        &self,
        epoch: StreamEpoch,
        colors: EdgeColors,
    ) -> Result<OutputApplied, ControlError> {
        self.request(|reply| Command::Hold(epoch, colors, reply))
            .await
    }
    async fn finish(
        &self,
        session: SessionId,
        mode: IdleOutputMode,
    ) -> Result<OutputApplied, ControlError> {
        self.request(|reply| Command::Finish(session, mode, reply))
            .await
    }
    async fn observe(&self) -> Result<Observation<OutputState>, ControlError> {
        self.mailbox.observe()
    }
}
#[derive(Clone)]
pub struct WledColors(Arc<Mailbox>);
impl ColorSink for WledColors {
    fn offer(&self, frame: ColorFrame) -> Result<OfferResult, ControlError> {
        self.0.offer(frame)
    }
}

/// Fixed-width LED counts have the same JSON range on every host architecture.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "wire", serde(deny_unknown_fields))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct WledSettings {
    #[cfg_attr(feature = "schema", schema(value_type = String))]
    pub address: std::net::IpAddr,
    #[cfg_attr(feature = "schema", schema(schema_with = led_counts_schema))]
    pub leds: [u32; 4],
}

#[cfg(feature = "schema")]
fn led_counts_schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
    use utoipa::openapi::schema::{ArrayBuilder, ObjectBuilder, Type};
    ArrayBuilder::new()
        .prefix_items((0..4).map(|_| {
            ObjectBuilder::new()
                .schema_type(Type::Integer)
                .minimum(Some(0.0))
                .maximum(Some(f64::from(u32::MAX)))
                .build()
        }))
        .min_items(Some(4))
        .max_items(Some(4))
        .into()
}

#[cfg(feature = "schema")]
pub mod schema;

impl WledSettings {
    fn validate(&self) -> Result<(), ControlError> {
        let invalid_address = self.address.is_unspecified()
            || self.address.is_multicast()
            || match self.address {
                std::net::IpAddr::V4(ip) => ip.is_broadcast(),
                std::net::IpAddr::V6(ip) => ip.to_ipv4_mapped().is_some_and(|ip| {
                    ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast()
                }),
            };
        if invalid_address {
            return Err(ControlError::Invalid {
                field: "address",
                reason: "a unicast device address is required".into(),
            });
        }
        let count: u64 = self.leds.iter().map(|count| u64::from(*count)).sum();
        if !(1..=u64::from(u16::MAX)).contains(&count) {
            return Err(ControlError::Invalid {
                field: "leds",
                reason: "total LED count must be between 1 and 65535".into(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
