use crate::engine::{Core, settings::lock};
use crate::intent::{Mode, Selection};
use crate::session_lifecycle::SessionPhase;
use prysm_ports::{
    Availability, DeviceReport, Input, LightingTarget, OutputIssue, SessionState, WaitingFor,
};

impl Core {
    pub(in crate::engine) fn publish_lighting(&mut self) {
        let lighting = &self.lighting;
        let selection = lighting.intent.selection();
        let target = match selection {
            Selection::Mode(Mode::ScreenSync) => LightingTarget::ScreenSync,
            Selection::Mode(Mode::Effects) => LightingTarget::Effects,
            Selection::Off | Selection::External => LightingTarget::Off,
        };
        let running = lighting.lifecycle.phase() == SessionPhase::Running;
        let waiting_for_output = matches!(selection, Selection::Mode(_))
            && lighting.output.available() == Availability::Waiting;
        let reason = || {
            lighting
                .intent
                .error()
                .cloned()
                .or_else(|| lighting.failure.clone())
                .or_else(|| self.unreadable.clone())
                .or_else(|| {
                    if target == LightingTarget::ScreenSync {
                        lighting.capture.reason().cloned()
                    } else {
                        None
                    }
                })
                .or_else(|| lighting.output.reason().cloned())
                .or_else(|| lighting.settings_error.clone())
        };
        let mut control = lighting.control();
        let state = match lighting.fault() {
            Some(phase) => SessionState::Faulted(phase),
            None if running => SessionState::ScreenSync,
            None if waiting_for_output => SessionState::Unanswered { failure: reason() },
            None if target == LightingTarget::Effects
                && lighting.active.is_none()
                && !control.busy
                && lighting.output.available() == Availability::Ready =>
            {
                SessionState::Effects { failure: reason() }
            }
            None => SessionState::Stopped { failure: reason() },
        };
        control.waiting = if target == LightingTarget::ScreenSync
            && lighting.capture.available() == Availability::Waiting
        {
            Some(WaitingFor::Capture)
        } else if waiting_for_output {
            Some(WaitingFor::Output)
        } else {
            None
        };
        self.state.send_modify(|snapshot| {
            snapshot.target = target;
            snapshot.session = state;
            snapshot.control = control;
            if running {
                snapshot.input.get_or_insert_with(Input::default);
            } else {
                snapshot.input = None;
                snapshot.device = if waiting_for_output {
                    DeviceReport {
                        issue: Some(OutputIssue::Unreachable),
                        ..DeviceReport::default()
                    }
                } else {
                    DeviceReport::default()
                };
            }
        });
    }

    pub(in crate::engine) fn measure(&mut self) {
        let lighting = &mut self.lighting;
        let (Some(meter), Some(session)) = (&mut lighting.meter, &lighting.active) else {
            return;
        };
        let input = Some(meter.sample(&self.stats));
        let device = session.observed();
        self.state.send_if_modified(|snapshot| {
            let changed = snapshot.input != input || snapshot.device != device;
            snapshot.input = input;
            snapshot.device = device;
            changed
        });
    }

    pub(in crate::engine) async fn persist_mode(&mut self) {
        let mode = self.lighting.intent.remembered();
        if Some(mode) == self.persisted_mode {
            return;
        }
        self.persisted_mode = Some(mode);
        let store = self.store.clone();
        if let Err(error) = self
            .run_blocking(move || lock(&store).save_mode(mode))
            .await
            .and_then(|result| result)
        {
            tracing::warn!(error = %prysm_ports::chain(&error), "the selected mode could not be saved");
        }
    }
}
