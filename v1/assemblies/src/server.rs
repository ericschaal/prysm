//! Source selection belongs to the server host, not either capture adapter.
use prysm_capture_device::DeviceCaptureSettings;
use prysm_remote_capture::RemoteCaptureSettings;

mod host;
mod lifecycle;
mod owners;
pub use host::{HostError, HostEvent};
pub use prysm_output_terminal::TerminalSettings;

use futures::Stream;
use prysm_application::{Application, policy::V1Policy};
use prysm_capture_device::DeviceCapture;
use prysm_contract::{
    ControlError, OwnerStopped,
    application::{LightingTarget, Settings},
    capture::CaptureControl,
    identity::InstanceId,
    output::OutputControl,
    storage::SettingsRepository,
};
use prysm_flow::{ProcessingExecution, ProcessingFlow};
use prysm_output_wled::{WledOutput, WledSettings};
use prysm_processor::PrysmProcessor;
use std::{future::Future, num::NonZeroU64};

pub type HeadlessSettings = Settings<DeviceCaptureSettings, WledSettings>;

/// Run one concrete composition. Keep this future alive through shutdown; cancelling
/// it cannot certify native release. Commands are consumed only after initialization.
pub async fn run_headless(
    settings: HeadlessSettings,
    commands: impl Stream<Item = LightingTarget> + Unpin + Send,
    shutdown: impl Future<Output = ()> + Send,
    report: impl FnMut(HostEvent),
) -> Result<OwnerStopped, HostError> {
    settings.validate().map_err(HostError::configuration)?;
    let instance = new_instance();
    let capture = DeviceCapture::new(instance);
    let output = WledOutput::new(instance);
    if let Some(settings) = &settings.capture {
        capture
            .control
            .validate(settings)
            .map_err(HostError::configuration)?;
    }
    if let Some(settings) = &settings.output {
        output
            .control
            .validate(settings)
            .map_err(HostError::configuration)?;
    }
    let flow = ProcessingFlow::connect(
        capture.frames,
        PrysmProcessor::new(&settings.processing),
        output.colors,
        ProcessingExecution::DedicatedThread,
    );
    let application = Application::new(
        capture.control,
        flow.control,
        output.control,
        FixedSettings(settings),
        V1Policy,
    );
    let owners = owners::Owners {
        output: owners::Task::spawn(|stop| output.owner.run(stop)),
        capture: owners::Task::spawn(|stop| capture.owner.run(stop)),
        flow: owners::Task::spawn(|stop| flow.owner.run(instance, stop)),
        application: owners::Task::spawn(|stop| application.owner.run(instance, stop)),
        output_shutdown: output.shutdown.into(),
    };
    host::run(owners, application.control, commands, shutdown, report).await
}

/// Run device capture into a terminal screen. Diagnostics are presented by its owner;
/// events after screen closure go to `fallback`. Retain through shutdown.
pub async fn run_terminal(
    settings: Settings<DeviceCaptureSettings, TerminalSettings>,
    commands: impl Stream<Item = LightingTarget> + Unpin + Send,
    shutdown: impl Future<Output = ()> + Send,
    mut fallback: impl FnMut(HostEvent),
) -> Result<OwnerStopped, HostError> {
    settings.validate().map_err(HostError::configuration)?;
    let instance = new_instance();
    let capture = DeviceCapture::new(instance);
    let output = prysm_output_terminal::TerminalOutput::new(instance);
    if let Some(settings) = &settings.capture {
        capture
            .control
            .validate(settings)
            .map_err(HostError::configuration)?;
    }
    if let Some(settings) = &settings.output {
        output
            .control
            .validate(settings)
            .map_err(HostError::configuration)?;
    }
    let flow = ProcessingFlow::connect(
        capture.frames,
        PrysmProcessor::new(&settings.processing),
        output.colors,
        ProcessingExecution::DedicatedThread,
    );
    let output_owner = output.owner.with_preview(&flow.preview).await;
    let application = Application::new(
        capture.control,
        flow.control,
        output.control,
        FixedSettings(settings),
        V1Policy,
    );
    let diagnostics = output.diagnostics;
    let owners = owners::Owners {
        output: owners::Task::spawn(|stop| output_owner.run(stop)),
        capture: owners::Task::spawn(|stop| capture.owner.run(stop)),
        flow: owners::Task::spawn(|stop| flow.owner.run(instance, stop)),
        application: owners::Task::spawn(|stop| application.owner.run(instance, stop)),
        output_shutdown: output.shutdown.into(),
    };
    host::run(owners, application.control, commands, shutdown, |event| {
        use prysm_output_terminal::HostDiagnostic;
        let shown = match &event {
            HostEvent::Application(update) => diagnostics.application(update),
            HostEvent::Ready(instance) => diagnostics.host(HostDiagnostic::Ready(*instance)),
            HostEvent::Stopping => diagnostics.host(HostDiagnostic::Stopping),
            HostEvent::Accepted(accepted) => diagnostics.host(HostDiagnostic::Accepted(accepted)),
            HostEvent::CommandRejected(failure) => {
                diagnostics.host(HostDiagnostic::CommandRejected(failure))
            }
            HostEvent::Failure(failure) => diagnostics.host(HostDiagnostic::Failure(failure)),
            HostEvent::CleanupOverdue { pending } => {
                diagnostics.host(HostDiagnostic::CleanupOverdue { pending })
            }
        };
        if !shown {
            fallback(event);
        }
    })
    .await
}

struct FixedSettings<S>(S);
impl<S: Clone + Send + Sync + 'static> SettingsRepository<S> for FixedSettings<S> {
    async fn load(&self) -> Result<Option<S>, ControlError> {
        Ok(Some(self.0.clone()))
    }
    async fn save(&self, _: S) -> Result<(), ControlError> {
        Err(ControlError::Unavailable("headless settings are read-only"))
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "wire",
    serde(tag = "adapter", content = "settings", rename_all = "snake_case")
)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ServerCaptureSettings {
    Device(DeviceCaptureSettings),
    Remote(RemoteCaptureSettings),
}

#[cfg(feature = "schema")]
pub fn document() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(components(schemas(ServerCaptureSettings)))]
    struct Contract;
    <Contract as utoipa::OpenApi>::openapi()
}

fn new_instance() -> InstanceId {
    // RandomState is independently seeded by std; IDs are lineage, not credentials.
    {
        use std::hash::{BuildHasher, Hasher};
        let value = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        InstanceId(NonZeroU64::new(value).unwrap_or(NonZeroU64::MIN))
    }
}
