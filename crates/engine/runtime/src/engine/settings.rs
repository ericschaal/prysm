//! The saved settings and what the processor is given: saves, the
//! calibration pattern, tried processing, and the config they make.
use super::Core;
use super::lighting::Lighting;
use prysm_core::Config;
use prysm_ports::{
    AdapterSettings, CalibrationPattern, Error, LightingTarget, Processing, Result, Revision,
    Section, Settings, SettingsStore, ValidationError,
};
use std::hash::{BuildHasher, RandomState};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::time::Instant;

/// A calibration pattern nobody has set again for this long is cleared, so a
/// client that vanished does not leave lighting unsmoothed for good.
pub const CALIBRATION_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Tried processing nobody tries again or saves for this long is dropped, so
/// a client that vanished mid-drag does not leave the lights unlike the
/// saved settings.
pub const TRIAL_TIMEOUT: Duration = Duration::from_secs(30);

/// A random start per process, so a revision a client kept from before a
/// restart is stale rather than matching. Below 2^52: browsers read revisions
/// as exact JSON numbers, and `Revision::next` has 2^52 steps before either limit.
pub(super) fn first_revision() -> Revision {
    Revision(RandomState::new().hash_one(0_u8) >> 12)
}

pub(super) type Store = Arc<Mutex<Box<dyn SettingsStore>>>;

pub(super) fn lock(store: &Store) -> std::sync::MutexGuard<'_, Box<dyn SettingsStore>> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Core {
    pub(super) async fn replace_settings(
        &mut self,
        expected: Revision,
        settings: Settings,
    ) -> Result<Revision> {
        let current = self.state.borrow().revision;
        if expected != current {
            return Err(Error::StaleRevision { expected, current });
        }
        self.registry.validate(&settings)?;
        let store = self.store.clone();
        let saved = settings.clone();
        self.run_blocking(move || lock(&store).save(&saved))
            .await??;
        self.unreadable = None;

        let (output_changed, capture_changed) = {
            let snapshot = self.state.borrow();
            (
                snapshot.settings.output != settings.output,
                snapshot.settings.capture != settings.capture,
            )
        };
        let adapters_changed = output_changed || capture_changed;
        let revision = current.next();
        // Saved settings end a trial.
        let install = |core: &mut Self| {
            core.trial = None;
            core.state.send_modify(|s| {
                s.settings = settings;
                s.revision = revision;
            });
        };
        if matches!(self.lighting, Lighting::ScreenSync(_)) && !adapters_changed {
            // Same adapters: the running session takes the new processing.
            self.reconfigure(install);
        } else {
            install(self);
        }
        let applied = self.reconcile(adapters_changed, output_changed).await;
        self.record(&applied);
        Ok(revision)
    }

    pub(super) fn set_calibration(
        &mut self,
        calibration: Option<CalibrationPattern>,
    ) -> Result<()> {
        if calibration.is_some() && self.state.borrow().target != LightingTarget::ScreenSync {
            return Err(Error::NeedsScreenSync);
        }
        self.calibrate(calibration);
        Ok(())
    }

    pub(super) fn try_processing(&mut self, processing: Processing) -> Result<()> {
        processing.validate()?;
        self.reconfigure(|core| core.trial = Some((processing, Instant::now() + TRIAL_TIMEOUT)));
        Ok(())
    }

    /// Publish `calibration`, restart its timeout and give a running session
    /// the processing config it implies.
    pub(super) fn calibrate(&mut self, calibration: Option<CalibrationPattern>) {
        self.calibration_expires = calibration.map(|_| Instant::now() + CALIBRATION_TIMEOUT);
        self.reconfigure(|core| {
            core.state.send_if_modified(|s| {
                let changed = s.calibration != calibration;
                s.calibration = calibration;
                changed
            });
        });
    }

    /// Apply `change`, then give a running session the config that follows
    /// if it differs. Without a session there is nothing to compare.
    pub(super) fn reconfigure(&mut self, change: impl FnOnce(&mut Self)) {
        if !matches!(self.lighting, Lighting::ScreenSync(_)) {
            change(self);
            return;
        }
        let before = self.config();
        change(self);
        let after = self.config();
        if let Lighting::ScreenSync(running) = &self.lighting
            && after != before
        {
            running.session.configure(after);
        }
    }

    /// The processor config: the saved processing or the one being tried,
    /// adjusted while a calibration pattern shows.
    pub(super) fn config(&self) -> Config {
        let snapshot = self.state.borrow();
        let processing = self
            .trial
            .as_ref()
            .map_or(&snapshot.settings.processing, |(processing, _)| processing);
        match snapshot.calibration {
            Some(calibration) => calibration.adjust(processing.config()),
            None => processing.config(),
        }
    }

    /// `section`'s adapter settings. Missing, they are not configured; while
    /// the store could not be read, its failure says why.
    pub(super) fn configured(
        &self,
        settings: Option<&AdapterSettings>,
        section: Section,
    ) -> Result<AdapterSettings> {
        match (settings, &self.unreadable) {
            (Some(settings), _) => Ok(settings.clone()),
            (None, Some(unreadable)) => Err(unreadable.clone()),
            (None, None) => Err(ValidationError::new(section.as_str(), "not configured").into()),
        }
    }
}
