//! Settings entry points; restart and cleanup transitions live in the lifecycle model.
use super::*;
use prysm_contract::identity::SettingsRevision;

type Value<C, O> = Settings<<C as CaptureControl>::Settings, <O as OutputControl>::Settings>;
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
        self.devices.stage(settings, revision);
        if same_adapters && self.model.is_active() {
            self.processing_update = Some(ProcessingUpdate {
                call: self.devices.update_processing(),
                interrupted: false,
            });
        } else {
            self.model.restart_settings(target);
        }
        Outcome::Changed.into()
    }
    pub(super) fn processing_updated(&mut self, result: Result<(), ControlError>) -> Progress {
        let update = self
            .processing_update
            .take()
            .expect("completed processing update");
        match result {
            Ok(()) => {
                if !update.interrupted {
                    self.devices.install_staged();
                }
                Progress {
                    outcome: Outcome::Changed,
                    settings: Some(Ok(())),
                }
            }
            Err(error) => {
                let failure = Failure::from(error);
                let progress = if update.interrupted {
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
