//! Synchronous transition rules. See ../../TRANSITIONS.md; no futures or ports here.
use prysm_contract::{
    ControlError, Failure,
    application::{LightingTarget, SessionState},
    capture::CaptureState,
    identity::{InstanceId, SessionId, StreamEpoch},
    output::IdleOutputMode,
};
use std::num::NonZeroU64;

use super::{Outcome, Requested};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    ConfigureProcessing,
    ConfigureOutput,
    Prepare(StreamEpoch),
    Open(SessionId),
    Admit(StreamEpoch),
    Hold(StreamEpoch),
    CloseAdmission(StreamEpoch),
    StopCapture(SessionId),
    Barrier(SessionId),
    Finish(SessionId, IdleOutputMode),
}
impl Action {
    fn cleanup(self) -> bool {
        matches!(
            self,
            Self::CloseAdmission(_) | Self::StopCapture(_) | Self::Barrier(_) | Self::Finish(..)
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Availability {
    Waiting(StreamEpoch),
    Available(StreamEpoch),
}
impl Availability {
    fn epoch(self) -> StreamEpoch {
        match self {
            Self::Waiting(epoch) | Self::Available(epoch) => epoch,
        }
    }
    fn action(self) -> Action {
        match self {
            Self::Waiting(epoch) => Action::Hold(epoch),
            Self::Available(epoch) => Action::Admit(epoch),
        }
    }
}

struct Session {
    id: SessionId,
    source: Availability,
    capture_owned: bool,
    output_owned: bool,
}
impl Session {
    fn new(id: SessionId, output_owned: bool) -> Self {
        Self {
            id,
            source: Availability::Waiting(StreamEpoch {
                session: id,
                generation: 0,
            }),
            capture_owned: false,
            output_owned,
        }
    }
}

#[derive(Clone, Copy)]
enum Preparation {
    Processing,
    Output,
    Black,
    Open,
    IdleMode,
}
#[derive(Clone, Copy)]
enum CleanupStage {
    Hold,
    Capture,
    Barrier,
    Finish,
}
struct Cleanup {
    session: Session,
    mode: IdleOutputMode,
    stage: CleanupStage,
    failure: Option<Failure>,
}
impl Cleanup {
    fn new(session: Session, mode: IdleOutputMode, failure: Option<Failure>) -> Self {
        Self {
            session,
            mode,
            failure,
            stage: CleanupStage::Hold,
        }
    }
}

enum Phase {
    Idle {
        mode: IdleOutputMode,
        failure: Option<(SessionId, Failure)>,
    },
    Starting {
        session: Session,
        target: LightingTarget,
        stage: Preparation,
    },
    Active {
        session: Session,
        applied: Availability,
    },
    Stopping(Cleanup),
    Blocked(Cleanup),
}

pub(super) struct Model {
    instance: InstanceId,
    next_session: u64,
    phase: Phase,
}
impl Model {
    pub fn new(instance: InstanceId) -> Self {
        Self {
            instance,
            next_session: 1,
            phase: Self::idle(),
        }
    }
    fn idle() -> Phase {
        Phase::Idle {
            mode: IdleOutputMode::Off,
            failure: None,
        }
    }
    fn allocate(&mut self, output_owned: bool) -> Session {
        let sequence = NonZeroU64::new(self.next_session).expect("session sequence exhausted");
        self.next_session = self
            .next_session
            .checked_add(1)
            .expect("session sequence exhausted");
        Session::new(
            SessionId {
                instance: self.instance,
                sequence,
            },
            output_owned,
        )
    }
    pub fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle { .. })
    }
    pub fn is_stopping(&self) -> bool {
        matches!(self.phase, Phase::Stopping(_) | Phase::Blocked(_))
    }
    pub fn status(&self) -> SessionState {
        match &self.phase {
            Phase::Idle {
                failure: Some((session, failure)),
                ..
            } => SessionState::Failed {
                session: *session,
                failure: failure.clone(),
                cleanup_complete: true,
            },
            Phase::Idle { .. } => SessionState::Idle,
            Phase::Starting { session, .. } => SessionState::Starting(session.id),
            Phase::Active { session, applied }
                if matches!(session.source, Availability::Available(_))
                    && *applied == session.source =>
            {
                SessionState::Running(session.id)
            }
            Phase::Active { session, .. } => SessionState::Waiting(session.id),
            Phase::Stopping(cleanup) => SessionState::Stopping(cleanup.session.id),
            Phase::Blocked(cleanup) => SessionState::Failed {
                session: cleanup.session.id,
                failure: cleanup
                    .failure
                    .clone()
                    .expect("blocked cleanup has a failure"),
                cleanup_complete: false,
            },
        }
    }
    pub fn check_request(&self, target: LightingTarget) -> Result<(), ControlError> {
        match &self.phase {
            Phase::Blocked(cleanup) if target != LightingTarget::Off => {
                Err(ControlError::CleanupPending(cleanup.session.id))
            }
            Phase::Stopping(_) => Err(ControlError::Conflict("session operation is in progress")),
            Phase::Starting { .. } if target == LightingTarget::Processed => {
                Err(ControlError::Conflict("session operation is in progress"))
            }
            Phase::Starting { session, .. }
                if target == LightingTarget::DeviceNative && !session.output_owned =>
            {
                Err(ControlError::Conflict("output preparation has not started"))
            }
            _ => Ok(()),
        }
    }
    /// Called only after phase and settings validation; no device call starts here.
    pub fn request(&mut self, target: LightingTarget, has_output: bool) -> Requested {
        match &self.phase {
            Phase::Idle { mode, .. } => {
                if target == LightingTarget::Off && !has_output {
                    return Requested::Complete;
                }
                let output_owned = *mode == IdleOutputMode::Native;
                let session = self.allocate(output_owned);
                let stage = if target == LightingTarget::Processed {
                    Preparation::Processing
                } else {
                    Preparation::Output
                };
                self.phase = Phase::Starting {
                    session,
                    target,
                    stage,
                };
            }
            Phase::Active { .. } if target == LightingTarget::Processed => {
                return Requested::Complete;
            }
            _ => self.stop(
                if target == LightingTarget::DeviceNative {
                    IdleOutputMode::Native
                } else {
                    IdleOutputMode::Off
                },
                true,
            ),
        }
        Requested::Pending
    }
    /// A force-Off (shutdown/failure) can supersede an in-flight final mode.
    pub fn stop(&mut self, mode: IdleOutputMode, retry: bool) {
        self.phase = match std::mem::replace(&mut self.phase, Self::idle()) {
            Phase::Idle {
                mode: IdleOutputMode::Native,
                ..
            } => {
                let mut cleanup = Cleanup::new(self.allocate(true), mode, None);
                cleanup.stage = CleanupStage::Finish;
                Phase::Stopping(cleanup)
            }
            idle @ Phase::Idle { .. } => idle,
            Phase::Starting { session, .. } | Phase::Active { session, .. } => {
                Phase::Stopping(Cleanup::new(session, mode, None))
            }
            Phase::Stopping(mut cleanup) => {
                cleanup.mode = mode;
                Phase::Stopping(cleanup)
            }
            Phase::Blocked(mut cleanup) if retry => {
                cleanup.mode = mode;
                cleanup.stage = CleanupStage::Hold;
                cleanup.failure = None;
                Phase::Stopping(cleanup)
            }
            blocked @ Phase::Blocked(_) => blocked,
        };
    }
    pub fn fail(&mut self, failure: Failure) {
        self.stop(IdleOutputMode::Off, false);
        if let Phase::Stopping(cleanup) | Phase::Blocked(cleanup) = &mut self.phase {
            // Retain the latest diagnostic, never an unbounded failure history.
            cleanup.failure = Some(failure);
        }
    }
    pub fn capture_changed(&mut self, state: &CaptureState) -> bool {
        let session = match &mut self.phase {
            Phase::Starting { session, .. } | Phase::Active { session, .. } => session,
            Phase::Stopping(cleanup) | Phase::Blocked(cleanup) => &mut cleanup.session,
            Phase::Idle { .. } => return false,
        };
        if let CaptureState::Failed {
            session: id,
            failure,
            ..
        } = state
            && *id == session.id
        {
            self.fail(failure.clone());
            return true;
        }
        if let Phase::Starting { session, .. } | Phase::Active { session, .. } = &mut self.phase {
            let source = match state {
                CaptureState::Available(epoch) => Availability::Available(*epoch),
                CaptureState::Waiting { epoch, .. } => Availability::Waiting(*epoch),
                _ => return false,
            };
            if source.epoch().session == session.id
                && source.epoch().generation > session.source.epoch().generation
            {
                session.source = source;
            }
        }
        false
    }
    pub fn next_action(&self) -> Option<Action> {
        match &self.phase {
            Phase::Idle { .. } | Phase::Blocked(_) => None,
            Phase::Starting {
                session,
                target,
                stage,
            } => Some(match stage {
                Preparation::Processing => Action::ConfigureProcessing,
                Preparation::Output => Action::ConfigureOutput,
                Preparation::Black => Action::Prepare(StreamEpoch {
                    session: session.id,
                    generation: 0,
                }),
                Preparation::Open => Action::Open(session.id),
                Preparation::IdleMode => Action::Finish(
                    session.id,
                    if *target == LightingTarget::DeviceNative {
                        IdleOutputMode::Native
                    } else {
                        IdleOutputMode::Off
                    },
                ),
            }),
            Phase::Active { session, applied } => {
                (session.source != *applied).then(|| session.source.action())
            }
            Phase::Stopping(cleanup) => Some(match cleanup.stage {
                CleanupStage::Hold if cleanup.session.output_owned => {
                    Action::CloseAdmission(cleanup.session.source.epoch())
                }
                CleanupStage::Hold | CleanupStage::Capture if cleanup.session.capture_owned => {
                    Action::StopCapture(cleanup.session.id)
                }
                CleanupStage::Hold | CleanupStage::Capture | CleanupStage::Barrier => {
                    Action::Barrier(cleanup.session.id)
                }
                CleanupStage::Finish => Action::Finish(cleanup.session.id, cleanup.mode),
            }),
        }
    }
    pub fn scheduled(&mut self, action: Action) {
        if let Phase::Starting { session, .. } = &mut self.phase {
            match action {
                Action::ConfigureOutput => session.output_owned = true,
                Action::Open(_) => session.capture_owned = true,
                _ => {}
            }
        }
    }
    pub fn discard_unissued_open(&mut self) {
        if let Phase::Stopping(cleanup) = &mut self.phase {
            cleanup.session.capture_owned = false;
        }
    }
    pub fn completed(&mut self, action: Action, result: Result<(), ControlError>) -> Outcome {
        if let Err(error) = result {
            let failure = Failure {
                message: error.to_string(),
            };
            if !action.cleanup() {
                self.fail(failure);
                return Outcome::Failed;
            }
            let mut cleanup = match std::mem::replace(&mut self.phase, Self::idle()) {
                Phase::Stopping(cleanup) => cleanup,
                Phase::Starting {
                    session, target, ..
                } => Cleanup::new(
                    session,
                    if target == LightingTarget::DeviceNative {
                        IdleOutputMode::Native
                    } else {
                        IdleOutputMode::Off
                    },
                    None,
                ),
                _ => unreachable!("cleanup calls belong to an output transaction or Stop"),
            };
            cleanup.failure = Some(failure.clone());
            if !matches!(action, Action::CloseAdmission(_)) {
                self.phase = Phase::Blocked(cleanup);
                return Outcome::Blocked(failure);
            }
            // A failed black write does not cancel the requested final idle mode.
            self.phase = Phase::Stopping(cleanup);
        }
        let (phase, outcome) = match std::mem::replace(&mut self.phase, Self::idle()) {
            Phase::Starting {
                session,
                target,
                stage,
            } => stage.completed(session, target, action),
            Phase::Active { session, .. } => {
                let applied = match action {
                    Action::Admit(epoch) => Availability::Available(epoch),
                    Action::Hold(epoch) => Availability::Waiting(epoch),
                    _ => unreachable!("active session only reconciles admission"),
                };
                (Phase::Active { session, applied }, Outcome::Changed)
            }
            Phase::Stopping(cleanup) => cleanup.completed(action),
            _ => unreachable!("device completions belong to a live phase"),
        };
        self.phase = phase;
        outcome
    }
}

impl Preparation {
    fn completed(
        self,
        session: Session,
        target: LightingTarget,
        action: Action,
    ) -> (Phase, Outcome) {
        let stage = match (self, action) {
            (Self::Processing, Action::ConfigureProcessing) => Self::Output,
            (Self::Output, Action::ConfigureOutput) => {
                if target == LightingTarget::Processed {
                    Self::Black
                } else {
                    Self::IdleMode
                }
            }
            (Self::Black, Action::Prepare(_)) => Self::Open,
            (Self::Open, Action::Open(_)) => {
                let applied = Availability::Waiting(StreamEpoch {
                    session: session.id,
                    generation: 0,
                });
                return (Phase::Active { session, applied }, Outcome::Ready);
            }
            (Self::IdleMode, Action::Finish(_, mode)) => {
                return (
                    Phase::Idle {
                        mode,
                        failure: None,
                    },
                    Outcome::Stopped(None),
                );
            }
            _ => unreachable!("preparation completion matches the dispatched stage"),
        };
        (
            Phase::Starting {
                session,
                target,
                stage,
            },
            Outcome::Changed,
        )
    }
}

impl Cleanup {
    fn completed(mut self, action: Action) -> (Phase, Outcome) {
        match action {
            Action::CloseAdmission(_) => self.stage = CleanupStage::Capture,
            Action::StopCapture(_) => self.stage = CleanupStage::Barrier,
            Action::Barrier(_) => self.stage = CleanupStage::Finish,
            Action::Finish(_, mode) if mode == self.mode => return self.finished(),
            Action::Finish(..) => self.stage = CleanupStage::Finish,
            // An older preparation/admission completion cannot resume Start.
            _ => {}
        }
        if matches!(self.stage, CleanupStage::Finish) && !self.session.output_owned {
            debug_assert_eq!(self.mode, IdleOutputMode::Off);
            self.finished()
        } else {
            (Phase::Stopping(self), Outcome::Changed)
        }
    }
    fn finished(self) -> (Phase, Outcome) {
        let outcome = Outcome::Stopped(self.failure.clone());
        let phase = Phase::Idle {
            mode: self.mode,
            failure: self.failure.map(|failure| (self.session.id, failure)),
        };
        (phase, outcome)
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
