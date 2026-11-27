//! Dispatch mode intent and supervise adapter checks and power commands.
use super::{Effect, Event, Ports};
use crate::engine::Core;
use crate::intent::{IntentAction, Mode, Selection};
use crate::session::Session;
use crate::session_lifecycle::OutputRelease;
use prysm_ports::{
    Availability, Error, LightingTarget, OutputObservationState, Power, Result,
    SessionPhase as FaultPhase,
};

impl Core {
    pub(in crate::engine) fn set_target(&mut self, target: LightingTarget) {
        let before = self.lighting.intent.selection();
        let phase = self.lighting.lifecycle.phase();
        let intent = &mut self.lighting.intent;
        match target {
            LightingTarget::Off => intent.request_off(phase),
            LightingTarget::ScreenSync => intent.select(Mode::ScreenSync, phase),
            LightingTarget::Effects => intent.select(Mode::Effects, phase),
        }
        self.refresh_blocked_on_selection(before);
        self.drive_lighting();
    }

    pub(in crate::engine) fn resume_screen_sync(&mut self) {
        let before = self.lighting.intent.selection();
        self.lighting.intent.resume_screen_sync();
        self.refresh_blocked_on_selection(before);
        self.drive_lighting();
    }

    pub(in crate::engine) fn press_power(&mut self) {
        let before = self.lighting.intent.selection();
        self.lighting
            .intent
            .press_power(self.lighting.lifecycle.phase());
        self.refresh_blocked_on_selection(before);
        self.drive_lighting();
    }

    pub(in crate::engine) fn pause_lighting(&mut self) {
        self.lighting.intent.pause();
        self.drive_lighting();
    }

    pub(in crate::engine) fn recheck_lighting(&mut self) {
        if self.lighting.active.is_some() {
            return;
        }
        self.lighting.intent.recheck_requested();
        self.lighting.failure = None;
        let check_capture = self.lighting.intent.selection() != Selection::Mode(Mode::Effects);
        if check_capture {
            self.lighting.capture.reset();
        }
        self.lighting.output.reset();
        self.check_output();
        if check_capture {
            self.lighting.capture.check();
        }
        self.drive_lighting();
    }

    fn refresh_blocked_on_selection(&mut self, before: Selection) {
        let after = self.lighting.intent.selection();
        if after == before || !matches!(after, Selection::Mode(_)) {
            return;
        }
        if after == Selection::Mode(Mode::ScreenSync)
            && self.lighting.capture.available() == Availability::Blocked
        {
            self.lighting.capture.reset();
        }
        if self.lighting.output.available() == Availability::Blocked {
            self.lighting.output.reset();
        }
    }

    pub(in crate::engine) fn lighting_settings_changed(
        &mut self,
        ports: Ports,
        capture: bool,
        output: bool,
    ) {
        self.lighting.capture.port = ports.capture;
        self.lighting.output.port = ports.output;
        self.lighting.settings_error = ports.failure;
        if !capture && !output {
            return;
        }
        self.lighting.intent.recheck_requested();
        self.lighting.failure = None;
        let reenter = self.lighting.active.is_some()
            || self.lighting.command.is_some()
            || (self.lighting.intent.selection() == Selection::Mode(Mode::Effects)
                && self.lighting.output.available() == Availability::Ready);
        if capture {
            self.lighting.capture.reset();
        }
        if output {
            self.lighting.output.reset();
            self.lighting.intent.retire_power_command();
            self.lighting.intent.observe_power(None);
            if let Some(session) = &mut self.lighting.active {
                session.retire_output_observations();
            }
        }
        if reenter {
            self.lighting.intent.configuration_changed();
            self.stop_session(OutputRelease::Automatic, false);
        }
        if output {
            let supports_effects = self
                .state
                .borrow()
                .settings
                .output
                .as_ref()
                .is_none_or(|settings| self.registry.supports_effects(settings));
            self.lighting.intent.set_supports_effects(supports_effects);
            self.check_output();
        }
        if capture && self.lighting.intent.selection() != Selection::Mode(Mode::Effects) {
            self.lighting.capture.check();
        }
        self.drive_lighting();
    }

    pub(in crate::engine) fn check_output(&mut self) {
        if self.lighting.output.port.is_err() {
            self.lighting
                .intent
                .observe_startup(OutputObservationState::Unsupported);
        }
        self.lighting.output.check();
    }

    pub(in crate::engine) fn drive_lighting(&mut self) {
        if self.lighting.fault().is_some() {
            self.calibrate(None);
            self.publish_lighting();
            return;
        }
        let old_target = self.state.borrow().target;
        loop {
            let Some(action) = self
                .lighting
                .intent
                .next_action(self.lighting.output.available())
            else {
                if self.drive_sync() {
                    continue;
                }
                break;
            };
            match action {
                IntentAction::StartSync => {
                    let failure = self.lighting.settings_error.clone();
                    if let Some(error) = failure {
                        self.lighting.failure = Some(error);
                        self.stop_sync_intent();
                        continue;
                    }
                    if self.lighting.capture.port.is_err() {
                        self.lighting.capture.check();
                    }
                    if self.lighting.output.port.is_err() {
                        self.check_output();
                    }
                }
                IntentAction::ExitMode => {
                    self.lighting.capture.cancel();
                    self.lighting.output.cancel();
                    let reset =
                        self.lighting.intent.selection() == Selection::Mode(Mode::ScreenSync);
                    self.lighting.capture.allow_open();
                    self.lighting.output.allow_open();
                    self.stop_session(
                        if reset {
                            OutputRelease::Automatic
                        } else {
                            OutputRelease::Preserve
                        },
                        false,
                    );
                    if self.lighting.active.is_none() {
                        self.lighting.intent.exited();
                    }
                }
                IntentAction::SendOff => self.send_power(Power::Off),
                IntentAction::ApplyEffects => self.send_power(Power::On),
            }
        }
        self.monitor();
        if old_target == LightingTarget::ScreenSync
            && self.lighting.intent.selection() != Selection::Mode(Mode::ScreenSync)
        {
            self.calibrate(None);
        }
        self.publish_lighting();
    }

    fn monitor(&mut self) {
        if self.lighting.command.is_some() {
            return;
        }
        let selection = self.lighting.intent.selection();
        let capture = selection == Selection::Mode(Mode::ScreenSync);
        let output =
            matches!(selection, Selection::Mode(_)) || self.lighting.intent.reconciling_startup();
        let exiting = self.lighting.intent.exiting();
        self.lighting.capture.monitor(capture && !exiting);
        self.lighting.output.monitor(output && !exiting);
    }

    fn send_power(&mut self, power: Power) {
        self.lighting.output.cancel();
        if self.state.borrow().settings.output.is_none() {
            self.lighting
                .intent
                .command_result(Ok(()), self.lighting.output.available());
            return;
        }
        let output = self.lighting.output.port.clone();
        self.lighting.command = Some(Effect::start(move |stop| output?.set_power(power, stop)));
    }

    pub(in crate::engine) async fn lighting_event(&mut self, event: Event) {
        match event {
            Event::Capture(Some((ready, _))) => self.lighting.capture.observe(ready),
            Event::Output(Some((ready, facts))) => {
                self.lighting.intent.observe_startup(facts);
                self.lighting.output.observe(ready);
            }
            Event::Capture(None) | Event::Output(None) => {}
            Event::Power(result) => self.power_finished(result),
            Event::Session(notice) => self.session_notice(notice).await,
            Event::StopTimedOut => {
                let phase = self
                    .lighting
                    .active
                    .as_ref()
                    .map_or(FaultPhase::Finishing, Session::deadline_phase);
                self.session_fault(Error::Faulted(phase));
            }
            Event::Measure => {
                self.measure();
                return;
            }
        }
        self.drive_lighting();
    }

    pub(super) fn power_finished(&mut self, result: Result<()>) {
        if self.lighting.intent.finish_retired_command() {
            return;
        }
        if let Err(error) = &result {
            self.lighting.output.failed(error.clone());
        }
        self.lighting
            .intent
            .command_result(result, self.lighting.output.available());
    }

    pub(super) fn stop_sync_intent(&mut self) {
        let lighting = &mut self.lighting;
        let failure = lighting
            .failure
            .as_ref()
            .or_else(|| lighting.capture.blocker())
            .or_else(|| lighting.output.blocker())
            .or_else(|| lighting.capture.reason())
            .or_else(|| lighting.output.reason())
            .cloned();
        lighting.intent.sync_stopped(failure);
    }
}
