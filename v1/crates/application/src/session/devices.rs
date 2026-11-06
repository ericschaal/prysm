//! Port calls and acknowledgment checks. This module makes no transition decisions.
use super::model::Action;
use futures::future::BoxFuture;
use prysm_contract::{
    ControlError,
    application::{LightingTarget, Settings},
    capture::{CaptureControl, CaptureStart},
    identity::SettingsRevision,
    output::OutputControl,
    processing::{ProcessingControl, ProcessingUpdate},
};
use prysm_core::EdgeColors;

type DeviceSettings<C, O> =
    Settings<<C as CaptureControl>::Settings, <O as OutputControl>::Settings>;

pub(super) struct Devices<C: CaptureControl, P, O: OutputControl> {
    capture: C,
    processing: P,
    output: O,
    settings: DeviceSettings<C, O>,
    revision: SettingsRevision,
    staged: Option<(DeviceSettings<C, O>, SettingsRevision)>,
    waiting_colors: EdgeColors,
}
impl<C: CaptureControl, P: ProcessingControl, O: OutputControl> Devices<C, P, O> {
    pub fn new(
        controls: (C, P, O),
        settings: DeviceSettings<C, O>,
        waiting_colors: EdgeColors,
    ) -> Self {
        let (capture, processing, output) = controls;
        Self {
            capture,
            processing,
            output,
            settings,
            revision: SettingsRevision(0),
            staged: None,
            waiting_colors,
        }
    }
    pub fn settings(&self) -> &DeviceSettings<C, O> {
        &self.settings
    }
    pub fn install(&mut self, settings: DeviceSettings<C, O>, revision: SettingsRevision) {
        self.settings = settings;
        self.revision = revision;
    }
    pub fn stage(&mut self, settings: DeviceSettings<C, O>, revision: SettingsRevision) {
        assert!(self.staged.is_none(), "one settings application at a time");
        self.staged = Some((settings, revision));
    }
    pub fn install_staged(&mut self) {
        let (settings, revision) = self.staged.take().expect("staged settings");
        self.install(settings, revision);
    }
    pub fn discard_staged(&mut self) {
        self.staged = None;
    }
    pub fn update_processing(&self) -> Pending {
        let (settings, revision) = self.staged.as_ref().expect("staged settings");
        self.configure_processing(settings, *revision)
    }
    pub fn validate(&self, target: LightingTarget) -> Result<(), ControlError> {
        self.validate_settings(&self.settings, target)
    }
    pub fn validate_settings(
        &self,
        settings: &DeviceSettings<C, O>,
        target: LightingTarget,
    ) -> Result<(), ControlError> {
        settings.validate()?;
        if let Some(capture) = &settings.capture {
            self.capture.validate(capture)?;
        }
        if let Some(output) = &settings.output {
            self.output.validate(output)?;
        }
        if target == LightingTarget::Processed && settings.capture.is_none() {
            return Err(ControlError::Invalid {
                field: "capture",
                reason: "capture settings are required".into(),
            });
        }
        if target != LightingTarget::Off && settings.output.is_none() {
            return Err(ControlError::Invalid {
                field: "output",
                reason: "output settings are required".into(),
            });
        }
        if target == LightingTarget::DeviceNative && !self.output.capabilities().native_mode {
            return Err(ControlError::Unavailable(
                "output does not support native mode",
            ));
        }
        Ok(())
    }
    fn configure_processing(
        &self,
        settings: &DeviceSettings<C, O>,
        revision: SettingsRevision,
    ) -> Pending {
        let processing = self.processing.clone();
        let mut config = settings.processing;
        if let Some(id) = settings.active_profile {
            config.correction = settings
                .profiles
                .iter()
                .find(|profile| profile.id == id)
                .expect("validated profile")
                .correction;
        }
        Pending {
            action: Action::ConfigureProcessing,
            started: false,
            future: Box::pin(async move {
                let reply = processing
                    .configure(ProcessingUpdate { revision, config })
                    .await?;
                if reply.revision != revision {
                    return Err(ControlError::Conflict(
                        "wrong processing configuration acknowledgment",
                    ));
                }
                Ok(())
            }),
        }
    }
    pub fn begin(&self, action: Action) -> Pending {
        if matches!(action, Action::ConfigureProcessing) {
            return self.configure_processing(&self.settings, self.revision);
        }
        let capture = self.capture.clone();
        let processing = self.processing.clone();
        let output = self.output.clone();
        let capture_settings = self.settings.capture.clone();
        let output_settings = self.settings.output.clone();
        let waiting_colors = self.waiting_colors.clone();
        let future = Box::pin(async move {
            match action {
                Action::InstallSettings | Action::ConfigureProcessing => {
                    unreachable!("handled above")
                }
                Action::ConfigureOutput => {
                    output
                        .configure(output_settings.expect("validated output settings"))
                        .await?;
                }
                Action::Prepare(epoch) | Action::CloseAdmission(epoch) => {
                    output.hold(epoch, EdgeColors::default()).await?;
                }
                Action::Open(session) => {
                    let reply = capture
                        .start(CaptureStart {
                            session,
                            settings: capture_settings.expect("validated capture settings"),
                        })
                        .await?;
                    if reply.session != session {
                        return Err(ControlError::Conflict("wrong capture start acknowledgment"));
                    }
                }
                Action::Admit(epoch) => {
                    output.admit(epoch).await?;
                }
                Action::Hold(epoch) => {
                    output.hold(epoch, waiting_colors).await?;
                }
                Action::StopCapture(session) => {
                    let reply = capture.stop(session).await?;
                    if reply.session != session {
                        return Err(ControlError::Conflict("wrong capture stop acknowledgment"));
                    }
                }
                Action::Barrier(session) => {
                    let reply = processing.barrier(session).await?;
                    if reply.session != session {
                        return Err(ControlError::Conflict(
                            "wrong processing barrier acknowledgment",
                        ));
                    }
                }
                Action::Finish(session, mode) => {
                    output.finish(session, mode).await?;
                }
            }
            Ok(())
        });
        Pending {
            action,
            future,
            started: false,
        }
    }
}

pub(super) struct Pending {
    pub action: Action,
    future: BoxFuture<'static, Result<(), ControlError>>,
    started: bool,
}
impl Pending {
    pub fn started(&self) -> bool {
        self.started
    }
    pub async fn wait(&mut self) -> Result<(), ControlError> {
        self.started = true;
        self.future.as_mut().await
    }
    pub fn interrupt_open(self) -> Option<BoxFuture<'static, Result<(), ControlError>>> {
        // An unpolled call must not first open capture after Stop has completed.
        self.started.then_some(self.future)
    }
}
