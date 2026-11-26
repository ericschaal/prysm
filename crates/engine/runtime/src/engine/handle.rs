//! What clients call: [`EngineHandle`], and the commands it sends the engine.
use super::lookups::Lookups;
use crate::feeds::{ColorsWatch, Feeds, PreviewWatch};
use crate::stats::{Counters, Stats};
use prysm_ports::{
    AdapterSettings, CalibrationPattern, Candidate, Error, LightingTarget, Modes, Processing,
    Registry, Result, Revision, Section, Settings, Snapshot,
};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch};

pub(super) type Reply<T> = oneshot::Sender<Result<T>>;

pub(super) enum Command {
    Request(Request),
    SurfaceFailed(Error),
    Shutdown(Reply<()>),
}

/// What a client asks of the lighting or the settings; refused unhandled
/// once the engine is faulted or shutting down (see [`Core::refusal`](super::Core::refusal)).
pub(super) enum Request {
    SetTarget(LightingTarget, Reply<()>),
    ReplaceSettings {
        expected: Revision,
        settings: Box<Settings>,
        reply: Reply<Revision>,
    },
    SetCalibration(Option<CalibrationPattern>, Reply<()>),
    TryProcessing(Box<Processing>, Reply<()>),
}

impl Request {
    /// Answer with `error` without acting.
    pub(super) fn refuse(self, error: Error) {
        match self {
            Self::SetTarget(_, reply)
            | Self::SetCalibration(_, reply)
            | Self::TryProcessing(_, reply) => {
                let _ = reply.send(Err(error));
            }
            Self::ReplaceSettings { reply, .. } => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

/// Cloneable access to a running engine.
#[derive(Clone)]
pub struct EngineHandle {
    pub(super) commands: mpsc::Sender<Command>,
    pub(super) snapshot: watch::Receiver<Snapshot>,
    pub(super) feeds: Feeds,
    pub(super) stats: Arc<Stats>,
    /// For [`candidates`](Self::candidates) and
    /// [`capture_modes`](Self::capture_modes), which need no engine state.
    pub(super) registry: Registry,
    pub(super) candidates: Lookups<(Section, String), Vec<Candidate>>,
    pub(super) modes: Lookups<AdapterSettings, Modes>,
}

impl EngineHandle {
    /// Lights that do not answer the request putting them `Off` or in their
    /// own effects are no failure: the target is taken and the engine waits
    /// for them ([`UNANSWERED_INTERVAL`](crate::UNANSWERED_INTERVAL)).
    ///
    /// # Errors
    /// Missing adapter settings, open failure, or a faulted or stopped engine.
    pub async fn set_target(&self, target: LightingTarget) -> Result<()> {
        self.call(|reply| Request::SetTarget(target, reply)).await
    }

    /// Validate, persist, then apply. Returns the new revision once persisted;
    /// a later apply failure is reported in the snapshot instead.
    ///
    /// # Errors
    /// Stale `expected`, invalid settings, a store failure, or a faulted engine.
    pub async fn replace_settings(
        &self,
        expected: Revision,
        settings: Settings,
    ) -> Result<Revision> {
        self.call(|reply| Request::ReplaceSettings {
            expected,
            settings: Box::new(settings),
            reply,
        })
        .await
    }

    /// Show a calibration pattern (`Some`), or end calibration (`None`).
    /// Setting one again, even unchanged, restarts its
    /// [`CALIBRATION_TIMEOUT`](super::CALIBRATION_TIMEOUT). It is cleared when the target leaves
    /// `ScreenSync` and at shutdown.
    ///
    /// # Errors
    /// [`Error::NeedsScreenSync`] for a pattern while the target is not
    /// `ScreenSync`, or a faulted or stopped engine.
    pub async fn set_calibration(&self, calibration: Option<CalibrationPattern>) -> Result<()> {
        self.call(|reply| Request::SetCalibration(calibration, reply))
            .await
    }

    /// Light with `processing` instead of the saved processing, without
    /// saving it or changing the revision: for a control being dragged, whose
    /// final value is then saved with [`replace_settings`](Self::replace_settings).
    /// A save ends the trial, and so does [`TRIAL_TIMEOUT`](super::TRIAL_TIMEOUT) without another
    /// call; the lights then follow the saved settings again. It is not in the
    /// snapshot and applies whatever the target (to a session started meanwhile).
    ///
    /// # Errors
    /// Invalid processing, or a faulted or stopped engine.
    pub async fn try_processing(&self, processing: Processing) -> Result<()> {
        self.call(|reply| Request::TryProcessing(Box::new(processing), reply))
            .await
    }

    /// What the adapter registered for `kind` in `section` finds to choose
    /// from (devices present, devices on the network). The lookup runs off
    /// the async runtime and bypasses the engine's mailbox: it is not state,
    /// and the adapter bounds it to about two seconds. One runs per section
    /// and kind at a time: who asks while it runs gets its answer.
    ///
    /// # Errors
    /// A kind not registered in `section` (field `<section>.kind`), or a
    /// panicking lookup.
    pub async fn candidates(&self, section: Section, kind: &str) -> Result<Vec<Candidate>> {
        let (registry, kind) = (self.registry.clone(), kind.to_owned());
        self.candidates
            .run((section, kind.clone()), move || {
                registry.candidates(section, &kind)
            })
            .await
    }

    /// What the capture `settings` select delivers (see
    /// [`SourceFactory::modes`](prysm_ports::SourceFactory::modes)). Like
    /// [`candidates`](Self::candidates), it runs off the async runtime,
    /// bypasses the engine's mailbox, and runs once at a time for the same
    /// settings, whose askers share its answer.
    ///
    /// # Errors
    /// An unregistered kind (field `capture.kind`), undecodable settings, or
    /// a panicking lookup.
    pub async fn capture_modes(&self, settings: AdapterSettings) -> Result<Modes> {
        let registry = self.registry.clone();
        self.modes
            .run(settings.clone(), move || registry.capture_modes(&settings))
            .await
    }

    #[must_use]
    pub fn snapshot(&self) -> watch::Receiver<Snapshot> {
        self.snapshot.clone()
    }

    /// Watch the latest frame and colors written by the running session. The
    /// session publishes previews only while at least one watch exists.
    #[must_use]
    pub fn preview(&self) -> PreviewWatch {
        self.feeds.preview.watch()
    }

    /// Watch the colors the running session last wrote to the LEDs. The
    /// session publishes them (a copy of the colors, never the frame) only
    /// while at least one watch exists, independently of preview watches.
    /// Empty while the LEDs show anything else: off, the device's own effects,
    /// or a source without frames.
    #[must_use]
    pub fn colors(&self) -> ColorsWatch {
        self.feeds.colors.watch()
    }

    /// Totals of frames, timings and opens since the process started, copied
    /// under a short lock without waking the engine.
    #[must_use]
    pub fn stats(&self) -> Counters {
        self.stats.read()
    }

    /// How many preview watches exist.
    #[cfg(any(test, feature = "testkit"))]
    #[must_use]
    pub fn preview_viewers(&self) -> usize {
        self.feeds.preview.watchers()
    }

    /// How many colors watches exist.
    #[cfg(any(test, feature = "testkit"))]
    #[must_use]
    pub fn colors_viewers(&self) -> usize {
        self.feeds.colors.watchers()
    }

    /// Report a failed surface, once the mailbox has room.
    pub(crate) async fn report_surface_failure(&self, error: Error) {
        let _ = self.commands.send(Command::SurfaceFailed(error)).await;
    }

    async fn call<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Request(request(reply)))
            .await
            .map_err(|_| Error::Closed)?;
        result.await.map_err(|_| Error::Closed)?
    }
}
