//! Lighting intent and mode ownership. The owner dispatches one action at a time.
//! Exiting a mode cancels its effects, retires their replies and finishes cleanup
//! before `exited` permits the next mode to enter.
use crate::session_lifecycle::SessionPhase;
use prysm_ports::{Availability, Error, OpenIntent, OutputDisplay, OutputObservationState, Power};

pub use prysm_ports::LightingMode as Mode;

/// The value for the Power button and mode selector. External displays as
/// On with neither mode selected; observed physical power is a separate fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Off,
    Mode(Mode),
    External,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Off { send: bool },
    External,
    Sync(OpenIntent),
    Effects { already_delegated: bool },
}

impl Request {
    fn selection(self) -> Selection {
        match self {
            Self::Off { .. } => Selection::Off,
            Self::External => Selection::External,
            Self::Sync(_) => Selection::Mode(Mode::ScreenSync),
            Self::Effects { .. } => Selection::Mode(Mode::Effects),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Scope {
    #[default]
    Idle,
    Sync,
    EffectsWaiting,
    ApplyingEffects,
    Delegated,
    SendingOff,
    Exiting,
    Faulted,
}

/// The host supplies effect results to the same owner. `StartSync` starts
/// the session decision machine, which gates device opens on both adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentAction {
    StartSync,
    ApplyEffects,
    SendOff,
    ExitMode,
}

#[derive(Debug)]
pub struct LightingIntent {
    remembered: Mode,
    supports_effects: bool,
    request: Request,
    scope: Scope,
    exit_required: bool,
    reconciling_boot: bool,
    physical: Option<Power>,
    error: Option<Error>,
}

impl LightingIntent {
    #[must_use]
    pub fn new(remembered: Option<Mode>, supports_effects: bool) -> Self {
        Self {
            remembered: if supports_effects {
                remembered.unwrap_or(Mode::Effects)
            } else {
                Mode::ScreenSync
            },
            supports_effects,
            request: Request::Off { send: false },
            scope: Scope::Idle,
            exit_required: false,
            reconciling_boot: true,
            physical: None,
            error: None,
        }
    }

    /// Persist this value on change. Storage belongs to the host.
    #[must_use]
    pub fn remembered(&self) -> Mode {
        self.remembered
    }

    #[must_use]
    pub fn selection(&self) -> Selection {
        self.request.selection()
    }

    #[must_use]
    pub fn physical_power(&self) -> Option<Power> {
        self.physical
    }

    #[must_use]
    pub fn reconciling_startup(&self) -> bool {
        self.reconciling_boot
    }

    pub fn recheck_requested(&mut self) {
        self.error = None;
        self.reconciling_boot = false;
    }

    pub fn observe_power(&mut self, power: Option<Power>) {
        self.physical = power;
    }

    #[must_use]
    pub fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    pub fn request_off(&mut self, session: SessionPhase) {
        if self.power_busy(session) || self.scope == Scope::Faulted {
            return;
        }
        let waiting = matches!(self.request, Request::Sync(_) | Request::Effects { .. })
            && (matches!(self.scope, Scope::EffectsWaiting | Scope::Idle)
                || (self.scope == Scope::Sync && session != SessionPhase::Running));
        self.replace(Request::Off {
            send: !(waiting && self.physical == Some(Power::Off)),
        });
    }

    #[must_use]
    pub fn off_pending(&self) -> bool {
        matches!(self.request, Request::Off { send: true })
    }

    #[must_use]
    fn power_busy(&self, session: SessionPhase) -> bool {
        matches!(self.scope, Scope::ApplyingEffects | Scope::SendingOff)
            || (self.scope == Scope::Sync && session == SessionPhase::OpeningOutput)
    }

    #[must_use]
    pub fn control(&self, session: SessionPhase) -> prysm_ports::LightingControl {
        prysm_ports::LightingControl {
            remembered: self.remembered,
            waiting: None,
            external: self.selection() == Selection::External,
            power: self.physical,
            busy: self.power_busy(session),
        }
    }

    pub fn set_supports_effects(&mut self, supported: bool) {
        self.supports_effects = supported;
        if !supported && self.remembered == Mode::Effects {
            self.remembered = Mode::ScreenSync;
            if self.selection() == Selection::Mode(Mode::Effects) {
                self.replace(Request::Sync(OpenIntent::Resume));
            }
        }
    }

    /// An unreleased local resource prevents all further device starts.
    pub fn fault(&mut self, error: Error) {
        self.scope = Scope::Faulted;
        self.request = Request::Off { send: false };
        self.reconciling_boot = false;
        self.error = Some(error);
    }

    /// Startup chooses at most one intent. Late observations update facts only.
    pub fn observe_startup(&mut self, observation: OutputObservationState) {
        let state = match observation {
            OutputObservationState::Unsupported => {
                self.reconciling_boot = false;
                return;
            }
            OutputObservationState::Pending => return,
            OutputObservationState::Observed(state) => state,
        };
        self.physical = Some(state.power);
        if !self.reconciling_boot {
            return;
        }
        self.reconciling_boot = false;
        if state.display == OutputDisplay::Override && self.supports_effects {
            self.remembered = Mode::Effects;
        }
        self.request = match (state.power, state.display, self.remembered) {
            (Power::Off, _, _) => Request::Off { send: false },
            (Power::On, OutputDisplay::Live, _) => Request::External,
            (Power::On, _, Mode::Effects) => Request::Effects {
                already_delegated: true,
            },
            (Power::On, _, Mode::ScreenSync) => Request::Sync(OpenIntent::Resume),
        };
    }

    fn replace(&mut self, request: Request) {
        self.reconciling_boot = false;
        self.error = None;
        self.request = request;
        self.exit_required |= self.scope != Scope::Idle;
    }

    /// Explicit mode selection, including a Power-on request.
    pub fn select(&mut self, mode: Mode, session: SessionPhase) {
        let handing_off = self.scope == Scope::Sync
            && session == SessionPhase::Running
            && self.physical == Some(Power::On);
        self.select_mode(mode, OpenIntent::Explicit, handing_off);
    }

    /// Continue an already authorized session after its host worker crashed.
    /// Only a fresh user selection grants permission to clear a device override.
    pub fn resume_screen_sync(&mut self) {
        self.select_mode(Mode::ScreenSync, OpenIntent::Resume, false);
    }

    fn select_mode(&mut self, mode: Mode, intent: OpenIntent, handing_off: bool) {
        if mode == Mode::Effects && !self.supports_effects {
            return;
        }
        self.reconciling_boot = false;
        if self.selection() == Selection::Mode(mode) || self.scope == Scope::Faulted {
            return;
        }
        self.remembered = mode;
        self.replace(match mode {
            Mode::ScreenSync => Request::Sync(intent),
            Mode::Effects => Request::Effects {
                already_delegated: handing_off,
            },
        });
    }

    /// Release without a remote power request (pause or stop).
    pub fn pause(&mut self) {
        self.replace(Request::Off { send: false });
    }

    /// Power remains actionable while awaiting readiness. A power command
    /// dispatched to the device must settle before another press is accepted.
    pub fn press_power(&mut self, session: SessionPhase) {
        if self.power_busy(session) || self.scope == Scope::Faulted {
            return;
        }
        if self.selection() == Selection::Off {
            self.select(self.remembered, session);
        } else {
            self.request_off(session);
        }
    }

    /// Called only for a changed adapter configuration. Re-enter a requested
    /// mode even when it needs an identical monitor after the change.
    pub fn configuration_changed(&mut self) {
        if matches!(self.selection(), Selection::Mode(_)) {
            self.exit_required |= self.scope != Scope::Idle;
            if let Request::Effects { already_delegated } = &mut self.request {
                *already_delegated = false;
            }
        }
    }

    /// A change to the configured output retires the reply, while the sent
    /// command still owns the device until its call settles.
    pub fn retire_power_command(&mut self) {
        if matches!(self.scope, Scope::ApplyingEffects | Scope::SendingOff) {
            self.exit_required = true;
        }
    }

    /// The command has settled after replacement. Neither its power facts
    /// nor its error belongs to the latest request.
    pub fn finish_retired_command(&mut self) -> bool {
        if !self.exit_required {
            return false;
        }
        self.exit_required = false;
        self.scope = Scope::Idle;
        true
    }

    /// The current session's output open completed. Future automatic resumes
    /// cannot replay its permission to clear an override.
    pub fn sync_started(&mut self) {
        if self.scope == Scope::Sync && !self.exit_required {
            self.request = Request::Sync(OpenIntent::Resume);
            self.physical = Some(Power::On);
        }
    }

    /// A confirmed external handoff chooses Effects without changing the
    /// device's chosen display. The runner retires old session observations.
    pub fn handed_off(&mut self) {
        if self.scope == Scope::Sync && !self.exit_required && self.supports_effects {
            self.remembered = Mode::Effects;
            self.replace(Request::Effects {
                already_delegated: true,
            });
        }
    }

    /// A required adapter blocks, or a session ends before stable operation.
    pub fn sync_stopped(&mut self, error: Option<Error>) {
        if self.scope == Scope::Sync && !self.exit_required {
            self.replace(Request::Off { send: false });
            self.error = error;
        }
    }

    /// Derive the next effect at dispatch time. Unsent commands can still be
    /// replaced. The caller drives again on observations or effect completion.
    pub fn next_action(&mut self, output: Availability) -> Option<IntentAction> {
        use IntentAction::*;
        if matches!(
            self.scope,
            Scope::ApplyingEffects | Scope::SendingOff | Scope::Exiting | Scope::Faulted
        ) {
            return None;
        }
        if self.exit_required {
            self.scope = Scope::Exiting;
            return Some(ExitMode);
        }
        match self.request {
            Request::Off { send: true } => {
                self.scope = Scope::SendingOff;
                self.request = Request::Off { send: false };
                Some(SendOff)
            }
            Request::Sync(_) if self.scope == Scope::Idle => {
                self.scope = Scope::Sync;
                Some(StartSync)
            }
            Request::Effects {
                already_delegated: true,
            } => {
                self.scope = Scope::Delegated;
                None
            }
            Request::Effects {
                already_delegated: false,
            } => match output {
                Availability::Ready => {
                    self.scope = Scope::ApplyingEffects;
                    Some(ApplyEffects)
                }
                Availability::Blocked => {
                    self.replace(Request::Off { send: false });
                    self.next_action(output)
                }
                _ if self.scope == Scope::Idle => {
                    self.scope = Scope::EffectsWaiting;
                    None
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Local cleanup for the old mode, including cancellation of its monitors.
    pub fn exited(&mut self) {
        debug_assert_eq!(self.scope, Scope::Exiting);
        self.exit_required = false;
        self.scope = Scope::Idle;
    }

    pub fn exiting(&self) -> bool {
        self.scope == Scope::Exiting
    }

    pub fn sync_open_intent(&self) -> Option<OpenIntent> {
        match self.request {
            Request::Sync(intent) if self.scope == Scope::Sync && !self.exit_required => {
                Some(intent)
            }
            _ => None,
        }
    }

    /// A sent command settled. An unanswered Effects request waits for the
    /// adapter's next Ready observation; Off is one-shot.
    pub fn command_result(&mut self, result: Result<(), Error>, output: Availability) {
        match self.scope {
            Scope::SendingOff => {
                self.scope = Scope::Idle;
                self.exit_required = false;
                self.physical = result.is_ok().then_some(Power::Off);
                self.error = result.err();
            }
            Scope::ApplyingEffects => {
                self.scope = Scope::Delegated;
                if result.is_ok() {
                    self.physical = Some(Power::On);
                    self.error = None;
                }
                match result {
                    Ok(()) => {
                        self.request = Request::Effects {
                            already_delegated: true,
                        }
                    }
                    Err(error) if output == Availability::Waiting => {
                        self.scope = Scope::Idle;
                        self.error = Some(error);
                    }
                    Err(error) => {
                        self.replace(Request::Off { send: false });
                        self.error = Some(error);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
