//! Device capture to WLED.
use crate::host::{self, HostError, HostEvent, Owners, SettingsOwner, Task, new_instance};
use futures::{Stream, StreamExt};
use prysm_application::{Application, policy::ApplicationPolicy};
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
pub async fn run(
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
        ApplicationPolicy::default(),
    );
    let settings_owner = SettingsOwner {
        task: Task::spawn(|stop| settings.owner.run(stop)),
        shutdown: settings.shutdown,
    };
    let owners = Owners {
        settings: Some(settings_owner),
        output: Task::spawn(|stop| output.owner.run(stop)),
        capture: Task::spawn(|stop| capture.owner.run(stop)),
        flow: Task::spawn(|stop| flow.owner.run(instance, stop)),
        application: Task::spawn(|stop| application.owner.run(instance, stop)),
        output_shutdown: output.shutdown.into(),
    };
    let commands = commands.map(|target| Ok(ApplicationCommand::SetTarget(target)));
    host::run(owners, application.control, commands, shutdown, report).await
}
