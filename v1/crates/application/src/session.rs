//! Encapsulated lighting lifecycle; driven by the application task, never spawned.
//! Transition rules are in model.rs; device calls and acknowledgments in devices.rs.
mod devices;
mod model;

use devices::{Devices, Pending};
use futures::future::BoxFuture;
use model::{Action, Model};
use prysm_contract::{
    ControlError, Failure,
    application::{LightingTarget, SessionState, Settings},
    capture::{CaptureControl, CaptureState},
    identity::InstanceId,
    output::{IdleOutputMode, OutputControl},
    processing::ProcessingControl,
};
use prysm_core::EdgeColors;

pub(super) enum Requested {
    Pending,
    Complete,
}
pub(super) enum Outcome {
    Changed,
    Ready,
    Failed,
    Stopped(Option<Failure>),
    Blocked(Failure),
}

pub(super) struct Lifecycle<C: CaptureControl, P, O: OutputControl> {
    model: Model,
    devices: Devices<C, P, O>,
    pending: Option<Pending>,
    opening: Option<BoxFuture<'static, Result<(), ControlError>>>,
}
impl<C: CaptureControl, P: ProcessingControl, O: OutputControl> Lifecycle<C, P, O> {
    pub fn new(
        instance: InstanceId,
        controls: (C, P, O),
        settings: Settings<C::Settings, O::Settings>,
        waiting_colors: EdgeColors,
    ) -> Self {
        Self {
            model: Model::new(instance),
            devices: Devices::new(controls, settings, waiting_colors),
            pending: None,
            opening: None,
        }
    }
    pub fn settings(&self) -> &Settings<C::Settings, O::Settings> {
        self.devices.settings()
    }
    pub fn status(&self) -> SessionState {
        self.model.status()
    }
    pub fn is_idle(&self) -> bool {
        self.model.is_idle()
    }
    pub fn request(&mut self, target: LightingTarget) -> Result<Requested, ControlError> {
        self.model.check_request(target)?;
        self.devices.validate(target)?;
        let result = self.model.request(target, self.settings().output.is_some());
        self.interrupt_open();
        Ok(result)
    }
    pub fn shutdown(&mut self) {
        self.model.stop(IdleOutputMode::Off, true);
        self.interrupt_open();
    }
    pub fn fail(&mut self, failure: Failure) {
        self.model.fail(failure);
        self.interrupt_open();
    }
    /// True means capture failed for the owned session and cleanup is now required.
    pub fn capture_changed(&mut self, state: &CaptureState) -> bool {
        let failed = self.model.capture_changed(state);
        self.interrupt_open();
        failed
    }
    fn interrupt_open(&mut self) {
        if self.model.is_stopping()
            && self
                .pending
                .as_ref()
                .is_some_and(|call| matches!(call.action, Action::Open(_)))
        {
            self.opening = self.pending.take().expect("capture open").interrupt_open();
            if self.opening.is_none() {
                self.model.discard_unissued_open();
            }
        }
    }
    /// Cancel-safe: select may drop this waiter, but both device futures stay here.
    /// Each completion emits one domain outcome; the coordinator never sees actions.
    pub async fn progress(&mut self) -> Outcome {
        if self.pending.is_none()
            && let Some(action) = self.model.next_action()
            && !(matches!(action, Action::Barrier(_)) && self.opening.is_some())
        {
            self.model.scheduled(action);
            self.pending = Some(self.devices.begin(action));
        }
        tokio::select! {
            result = async { self.pending.as_mut().expect("guarded").wait().await }, if self.pending.is_some() => {
                let action = self.pending.take().expect("completed call").action;
                self.model.completed(action, result)
            }
            _ = async { self.opening.as_mut().expect("guarded").as_mut().await }, if self.opening.is_some() => {
                // Cancellation may fail the open reply. Capture.stop certifies release.
                self.opening = None;
                Outcome::Changed
            }
            else => std::future::pending().await,
        }
    }
}
