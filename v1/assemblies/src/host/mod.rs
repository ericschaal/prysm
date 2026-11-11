//! Shared process supervision. The application owns lighting sessions.
mod connection;
mod lifecycle;
mod owners;

use connection::{Connection, Event};
use futures::Stream;
use lifecycle::Lifecycle;
#[cfg(any(feature = "headless", feature = "tui"))]
pub(crate) use owners::SettingsOwner;
use owners::{Owner, settings_completion};
pub(crate) use owners::{Owners, Task};
use prysm_contract::{
    ControlError, Failure, OwnerStopped,
    application::{
        Accepted, ApplicationCommand, ApplicationControl, ApplicationEvent, ApplicationSnapshot,
    },
    identity::InstanceId,
    observation::Update,
};
use std::future::{Future, pending};
use tokio::time::sleep_until;

#[derive(Debug)]
pub enum HostEvent {
    Ready(InstanceId),
    Stopping,
    Application(Box<Update<ApplicationSnapshot<()>, ApplicationEvent>>),
    Accepted(Accepted),
    CommandRejected(Failure),
    Failure(Failure),
    CleanupOverdue { pending: Vec<&'static str> },
}

#[derive(Debug)]
pub struct HostError {
    pub failures: Vec<Failure>,
}
impl HostError {
    #[cfg(any(feature = "headless", feature = "tui"))]
    pub(super) fn configuration(error: ControlError) -> Self {
        Self {
            failures: vec![error.into()],
        }
    }
}
impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, failure) in self.failures.iter().enumerate() {
            if index != 0 {
                write!(f, "; ")?;
            }
            write!(f, "{failure}")?;
        }
        Ok(())
    }
}
impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.failures.first().map(|failure| failure as _)
    }
}

pub(crate) async fn run<C: ApplicationControl>(
    mut owners: Owners,
    control: C,
    commands: impl Stream<Item = Result<ApplicationCommand<C::Settings>, ControlError>> + Unpin + Send,
    shutdown: impl Future<Output = ()> + Send,
    mut report: impl FnMut(HostEvent),
) -> Result<OwnerStopped, HostError> {
    let mut lifecycle = Lifecycle::new();
    let mut connection = Connection::new(control, commands);
    tokio::pin!(shutdown);
    loop {
        if owners.pending().is_empty() {
            return lifecycle.result();
        }
        tokio::select! {
            biased;
            result = owners.application.completion() => {
                lifecycle.on_owner_exited(Owner::Application, result, &mut owners, &mut report);
            },
            result = owners.capture.completion() => {
                lifecycle.on_owner_exited(Owner::Capture, result, &mut owners, &mut report);
            },
            result = owners.flow.completion() => {
                lifecycle.on_owner_exited(Owner::Processing, result, &mut owners, &mut report);
            },
            result = owners.output.completion() => {
                lifecycle.on_owner_exited(Owner::Output, result, &mut owners, &mut report);
            },
            result = settings_completion(&mut owners.settings) => {
                lifecycle.on_owner_exited(Owner::Settings, result, &mut owners, &mut report);
            },
            _ = &mut shutdown, if lifecycle.is_active() => {
                lifecycle.request_stop(&mut owners, &mut report);
            }
            _ = async {
                match lifecycle.deadline() {
                    Some(at) => sleep_until(at).await,
                    None => pending().await,
                }
            } => {
                lifecycle.on_timeout(&mut owners, &mut report);
            }
            event = connection.advance(), if lifecycle.is_active() => {
                match event {
                    Ok(Some(Event::Connected { instance, snapshot })) => {
                        report(HostEvent::Application(snapshot));
                        lifecycle.on_initialized(instance, &mut report);
                    }
                    Ok(Some(Event::Updated(update))) => report(HostEvent::Application(update)),
                    Ok(Some(Event::Accepted(accepted))) => report(HostEvent::Accepted(accepted)),
                    Ok(Some(Event::Rejected(failure))) => report(HostEvent::CommandRejected(failure)),
                    Ok(None) => {}
                    Err(error) => {
                        lifecycle.on_failure("application observation", error, &mut owners, &mut report);
                    }
                }
            }
        }
        if !lifecycle.is_active() {
            connection.close();
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(any(feature = "headless", feature = "tui"))]
pub(crate) fn new_instance() -> prysm_contract::identity::InstanceId {
    use prysm_contract::identity::InstanceId;
    use std::num::NonZeroU64;
    // RandomState is independently seeded by std; IDs are lineage, not credentials.
    {
        use std::hash::{BuildHasher, Hasher};
        let value = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        InstanceId(NonZeroU64::new(value).unwrap_or(NonZeroU64::MIN))
    }
}
