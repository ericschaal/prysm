//! Concrete host wiring; persistence is owned alongside device resources.
use super::{HostError, HostEvent, host, new_instance, owners};
use futures::{Stream, StreamExt};
use prysm_application::{Application, policy::V1Policy};
use prysm_capture_device::DeviceCapture;
use prysm_contract::{
    OwnerStopped,
    application::{ApplicationCommand, LightingTarget},
};
use prysm_flow::{ProcessingExecution, ProcessingFlow};
use prysm_processor::PrysmProcessor;
use std::future::Future;

/// Run one concrete composition. Keep this future alive through shutdown; cancelling
/// it cannot certify native release. Commands are consumed only after initialization.
#[cfg(feature = "headless")]
pub async fn run_headless(
    settings_path: std::path::PathBuf,
    commands: impl Stream<Item = LightingTarget> + Unpin + Send,
    shutdown: impl Future<Output = ()> + Send,
    report: impl FnMut(HostEvent),
) -> Result<OwnerStopped, HostError> {
    let instance = new_instance();
    let settings =
        prysm_settings_file::FileSettings::new(settings_path).map_err(HostError::configuration)?;
    let capture = DeviceCapture::new(instance);
    let output = prysm_output_wled::WledOutput::new(instance);
    let flow = ProcessingFlow::connect(
        capture.frames,
        PrysmProcessor::new(&Default::default()),
        output.colors,
        ProcessingExecution::DedicatedThread,
    );
    let application = Application::new(
        capture.control,
        flow.control,
        output.control,
        settings.repository,
        V1Policy,
    );
    let settings_owner = owners::SettingsOwner {
        task: owners::Task::spawn(|stop| settings.owner.run(stop)),
        shutdown: settings.shutdown,
    };
    let owners = owners::Owners {
        settings: Some(settings_owner),
        output: owners::Task::spawn(|stop| output.owner.run(stop)),
        capture: owners::Task::spawn(|stop| capture.owner.run(stop)),
        flow: owners::Task::spawn(|stop| flow.owner.run(instance, stop)),
        application: owners::Task::spawn(|stop| application.owner.run(instance, stop)),
        output_shutdown: output.shutdown.into(),
    };
    let commands = commands.map(|target| Ok(ApplicationCommand::SetTarget(target)));
    host::run(owners, application.control, commands, shutdown, report).await
}

/// Run device capture into a terminal screen. Diagnostics are presented by its owner;
/// events after screen closure go to `fallback`. Retain through shutdown.
#[cfg(feature = "tui")]
pub async fn run_tui(
    settings_path: std::path::PathBuf,
    commands: impl Stream<Item = LightingTarget> + Unpin + Send,
    shutdown: impl Future<Output = ()> + Send,
    mut fallback: impl FnMut(HostEvent),
) -> Result<OwnerStopped, HostError> {
    let instance = new_instance();
    let settings =
        prysm_settings_file::FileSettings::new(settings_path).map_err(HostError::configuration)?;
    let capture = DeviceCapture::new(instance);
    let output = prysm_output_terminal::TerminalOutput::new(instance);
    let flow = ProcessingFlow::connect(
        capture.frames,
        PrysmProcessor::new(&Default::default()),
        output.colors,
        ProcessingExecution::DedicatedThread,
    );
    let (input, receiver) = tokio::sync::mpsc::channel(16);
    let output_owner = output
        .owner
        .with_commands(input)
        .with_preview(&flow.preview)
        .await;
    let application = Application::new(
        capture.control,
        flow.control,
        output.control,
        settings.repository,
        V1Policy,
    );
    let terminal_commands = prysm_control_terminal::commands(application.control.clone(), receiver);
    let commands = futures::stream::select(
        commands.map(|target| Ok(ApplicationCommand::SetTarget(target))),
        terminal_commands,
    );
    let diagnostics = output.diagnostics;
    let settings_owner = owners::SettingsOwner {
        task: owners::Task::spawn(|stop| settings.owner.run(stop)),
        shutdown: settings.shutdown,
    };
    let owners = owners::Owners {
        settings: Some(settings_owner),
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
