//! Interprets the lighting machines and owns their effects.
mod adapter;
mod run;
mod session;
mod snapshot;

use crate::intent::LightingIntent;
use crate::meter::Meter;
use crate::session::{Notice, Session};
use crate::session_lifecycle::{OutputRelease, SessionLifecycle};
use adapter::{Adapter, Effect, Observed};
use prysm_ports::{
    ConfiguredOutput, ConfiguredSource, Error, Registry, Result, Section, SessionPhase, Settings,
    ValidationError,
};
use std::sync::Arc;
use tokio::time::Instant;

pub(super) struct Lighting {
    intent: LightingIntent,
    lifecycle: SessionLifecycle,
    capture: Adapter<dyn ConfiguredSource>,
    output: Adapter<dyn ConfiguredOutput>,
    active: Option<Session>,
    meter: Option<Meter>,
    command: Option<Effect<Result<()>>>,
    stopping_at: Option<Instant>,
    idle_mode: Option<OutputRelease>,
    failure: Option<Error>,
    settings_error: Option<Error>,
}

/// Prepare both ports before a settings save. The runner installs them after
/// persistence succeeds. On boot, invalid saved settings remain available for repair.
pub(super) struct Ports {
    capture: Result<Arc<dyn ConfiguredSource>>,
    output: Result<Arc<dyn ConfiguredOutput>>,
    pub(super) failure: Option<Error>,
}

impl Ports {
    pub(super) fn new(
        registry: &Registry,
        settings: &Settings,
        unreadable: Option<&Error>,
    ) -> Self {
        let capture = settings
            .capture
            .as_ref()
            .map(|raw| registry.source(raw))
            .transpose();
        let output = settings
            .output
            .as_ref()
            .map(|raw| registry.output(raw))
            .transpose();
        let failure = settings
            .processing
            .validate()
            .err()
            .map(Error::from)
            .or_else(|| capture.as_ref().err().cloned())
            .or_else(|| output.as_ref().err().cloned());
        let missing = |section: Section| {
            unreadable
                .cloned()
                .unwrap_or_else(|| ValidationError::new(section.as_str(), "not configured").into())
        };
        Self {
            capture: capture.and_then(|port| port.ok_or_else(|| missing(Section::Capture))),
            output: output.and_then(|port| port.ok_or_else(|| missing(Section::Output))),
            failure,
        }
    }
}

impl Lighting {
    pub(super) fn new(intent: LightingIntent, ports: Ports) -> Self {
        Self {
            intent,
            lifecycle: SessionLifecycle::default(),
            capture: Adapter::new(ports.capture),
            output: Adapter::new(ports.output),
            settings_error: ports.failure,
            active: None,
            meter: None,
            command: None,
            stopping_at: None,
            idle_mode: None,
            failure: None,
        }
    }

    pub(super) fn control(&self) -> prysm_ports::LightingControl {
        self.intent.control(self.lifecycle.phase())
    }

    pub(super) fn fault(&self) -> Option<SessionPhase> {
        match self.lifecycle.phase() {
            crate::session_lifecycle::SessionPhase::Faulted(phase) => Some(phase),
            _ => None,
        }
    }

    pub(super) fn session(&self) -> Option<&Session> {
        self.active.as_ref()
    }

    pub(super) async fn next(&mut self) -> Event {
        tokio::select! {
            notice = async {
                match self.active.as_mut() {
                    Some(active) => active.next().await,
                    None => std::future::pending().await,
                }
            } => Event::Session(notice),
            observed = self.capture.next() => Event::Capture(observed),
            observed = self.output.next() => Event::Output(observed),
            result = Effect::next(&mut self.command) => Event::Power(result.and_then(|result| result)),
            () = async {
                match self.stopping_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => Event::StopTimedOut,
            () = async {
                match self.meter.as_mut() {
                    Some(meter) => meter.tick().await,
                    None => std::future::pending().await,
                }
            } => Event::Measure,
        }
    }
}

pub(super) enum Event {
    Session(Notice),
    Capture(Observed),
    Output(Observed),
    Power(Result<()>),
    StopTimedOut,
    Measure,
}
