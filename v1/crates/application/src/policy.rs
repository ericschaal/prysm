//! Replaceable decisions. Resource cleanup and stale-write exclusion are contracts.
use prysm_contract::application::LightingTarget;
use prysm_core::EdgeColors;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureAction {
    AwaitCommand,
    RetryAfter(std::time::Duration),
}

#[derive(Debug, Clone)]
pub enum UnavailableAction {
    WaitHolding(EdgeColors),
    EndSession,
}

/// A host may supply different decisions without changing component ports.
pub trait ApplicationPolicy: Send + 'static {
    fn initial_target(&self) -> LightingTarget;
    fn terminal_failure(&self) -> FailureAction;
    fn input_unavailable(&self) -> UnavailableAction;
}

pub struct V1Policy;

impl ApplicationPolicy for V1Policy {
    fn initial_target(&self) -> LightingTarget {
        LightingTarget::Off
    }
    fn terminal_failure(&self) -> FailureAction {
        FailureAction::AwaitCommand
    }
    fn input_unavailable(&self) -> UnavailableAction {
        UnavailableAction::WaitHolding(EdgeColors::default())
    }
}
