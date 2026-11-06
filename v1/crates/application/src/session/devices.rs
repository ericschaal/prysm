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
            waiting_colors,
        }
    }
    pub fn settings(&self) -> &DeviceSettings<C, O> {
        &self.settings
    }
    pub fn validate(&self, target: LightingTarget) -> Result<(), ControlError> {
        if target == LightingTarget::Processed {
            self.capture.validate(self.settings.capture.as_ref().ok_or(
                ControlError::Invalid {
                    field: "capture",
                    reason: "capture settings are required".into(),
                },
            )?)?;
        }
        if target != LightingTarget::Off || self.settings.output.is_some() {
            self.output.validate(self.settings.output.as_ref().ok_or(
                ControlError::Invalid {
                    field: "output",
                    reason: "output settings are required".into(),
                },
            )?)?;
        }
        if target == LightingTarget::DeviceNative && !self.output.capabilities().native_mode {
            return Err(ControlError::Unavailable(
                "output does not support native mode",
            ));
        }
        Ok(())
    }
    pub fn begin(&self, action: Action) -> Pending {
        let capture = self.capture.clone();
        let processing = self.processing.clone();
        let output = self.output.clone();
        let capture_settings = self.settings.capture.clone();
        let output_settings = self.settings.output.clone();
        let mut config = self.settings.processing;
        if let Some(profile) = self
            .settings
            .active_profile
            .filter(|_| matches!(action, Action::ConfigureProcessing))
        {
            config.correction = self
                .settings
                .profiles
                .iter()
                .find(|candidate| candidate.id == profile)
                .expect("validated profile")
                .correction;
        }
        let waiting_colors = self.waiting_colors.clone();
        let future = Box::pin(async move {
            match action {
                Action::ConfigureProcessing => {
                    let revision = SettingsRevision(0);
                    let reply = processing
                        .configure(ProcessingUpdate { revision, config })
                        .await?;
                    if reply.revision != revision {
                        return Err(ControlError::Conflict(
                            "wrong processing configuration acknowledgment",
                        ));
                    }
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
    pub async fn wait(&mut self) -> Result<(), ControlError> {
        self.started = true;
        self.future.as_mut().await
    }
    pub fn interrupt_open(self) -> Option<BoxFuture<'static, Result<(), ControlError>>> {
        // An unpolled call must not first open capture after Stop has completed.
        self.started.then_some(self.future)
    }
}
