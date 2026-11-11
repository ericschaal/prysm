//! Settings entry points; restart and cleanup transitions live in the lifecycle model.
use super::*;
use prysm_contract::identity::SettingsRevision;

type Value<C, O> = Settings<<C as CaptureControl>::Settings, <O as OutputControl>::Settings>;
pub(super) enum SettingsApplication<S> {
    Restart(S, SettingsRevision),
    Processing {
        call: Pending,
        // Interruption discards installation intent while retaining the issued call.
        candidate: Option<(S, SettingsRevision)>,
    },
}

impl<C: CaptureControl, P: ProcessingControl, O: OutputControl> Lifecycle<C, P, O>
where
    C::Settings: PartialEq,
    O::Settings: PartialEq,
{
    pub fn validate_replacement(
        &self,
        settings: &Value<C, O>,
        target: LightingTarget,
    ) -> Result<(), ControlError> {
        self.model.check_replacement()?;
        self.devices.validate_settings(settings, target)
    }
    pub fn install_saved_if_off(&mut self, settings: &Value<C, O>, revision: SettingsRevision) {
        if self.model.is_off() {
            self.devices.install(settings.clone(), revision);
        }
    }
    pub fn replace(
        &mut self,
        settings: Value<C, O>,
        revision: SettingsRevision,
        target: LightingTarget,
    ) -> Progress {
        let same_adapters = self.settings().capture == settings.capture
            && self.settings().output == settings.output;
        if self.model.is_off() || (self.model.is_idle() && same_adapters) {
            self.devices.install(settings, revision);
            return Progress {
                outcome: Outcome::Changed,
                settings: Some(Ok(())),
            };
        }
        assert!(
            self.settings_application.is_none(),
            "one settings application at a time"
        );
        if same_adapters && self.model.is_active() {
            self.settings_application = Some(SettingsApplication::Processing {
                call: self.devices.configure_processing(&settings, revision),
                candidate: Some((settings, revision)),
            });
        } else {
            self.settings_application = Some(SettingsApplication::Restart(settings, revision));
            self.model.restart_settings(target);
        }
        Outcome::Changed.into()
    }
    pub(super) fn processing_updated(&mut self, result: Result<(), ControlError>) -> Progress {
        let Some(SettingsApplication::Processing { candidate, .. }) =
            self.settings_application.take()
        else {
            unreachable!("completed processing update")
        };
        match result {
            Ok(()) => {
                if let Some((settings, revision)) = candidate {
                    self.devices.install(settings, revision);
                }
                Progress {
                    outcome: Outcome::Changed,
                    settings: Some(Ok(())),
                }
            }
            Err(error) => {
                let failure = Failure::from(error);
                let progress = if candidate.is_none() {
                    self.model.fail(failure.clone());
                    Progress {
                        outcome: Outcome::Failed(failure.clone()),
                        settings: Some(Err(failure)),
                    }
                } else {
                    self.model.processing_update_failed(failure.clone());
                    Outcome::Failed(failure).into()
                };
                self.interrupt_pending_calls(progress)
            }
        }
    }
}

pub(crate) fn validate_settings<C: CaptureControl, O: OutputControl>(
    capture: &C,
    output: &O,
    settings: &Value<C, O>,
    target: LightingTarget,
) -> Result<(), ControlError> {
    settings.validate()?;
    if let Some(value) = &settings.capture {
        capture.validate(value)?;
    }
    if let Some(value) = &settings.output {
        output.validate(value)?;
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
    if target == LightingTarget::DeviceNative && !output.capabilities().native_mode {
        return Err(ControlError::Unavailable(
            "output does not support native mode",
        ));
    }
    Ok(())
}
