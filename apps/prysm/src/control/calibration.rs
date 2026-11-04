//! One temporary draft; the controller performs the requested runtime and disk work.
use prysm_api::CommandError;
use prysm_contract::{
    CalibrationProfile, CalibrationProfiles, CalibrationRequest, CalibrationStep, CalibrationView,
    Config, OperationId, SettingsView, UpdateSettings,
};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub(super) const LOCKED: &str =
    "Calibration in progress—finish or exit on the TV to change settings.";
const RECONNECT: Duration = Duration::from_secs(30);
const PAIRING: Duration = Duration::from_secs(120);

pub(super) enum Effect {
    None,
    Start,
    Save {
        update: Box<UpdateSettings>,
        profile: CalibrationProfile,
    },
    End(&'static str),
}

enum Outcome {
    Active,
    Saved(String),
    Discarded(String),
}

pub(super) struct Session {
    startup_cancel: CancellationToken,
    outcome: Outcome,
    started: bool,
    applied: Option<Config>,
    tv: OperationId,
    phone: Option<OperationId>,
    pairing: OperationId,
    draft: CalibrationProfile,
    replacing: bool,
    step: CalibrationStep,
    shade: u8,
    paused: bool,
    sequence: u32,
    compare_until: Instant,
    tv_seen: Instant,
    phone_seen: Instant,
    created: Instant,
}
impl Session {
    pub fn new(settings: &SettingsView, now: Instant) -> Self {
        Self {
            startup_cancel: CancellationToken::new(),
            outcome: Outcome::Active,
            started: false,
            applied: None,
            tv: OperationId::new(),
            phone: None,
            pairing: OperationId::new(),
            draft: Self::new_profile(&settings.settings.calibration),
            replacing: false,
            step: CalibrationStep::Choose,
            shade: 0,
            paused: false,
            sequence: 0,
            compare_until: now,
            tv_seen: now,
            phone_seen: now,
            created: now,
        }
    }

    fn new_profile(profiles: &CalibrationProfiles) -> CalibrationProfile {
        let mut number = 1;
        while profiles
            .profiles
            .iter()
            .any(|p| p.name == format!("Calibration {number}"))
        {
            number += 1;
        }
        CalibrationProfile {
            id: OperationId::new(),
            name: format!("Calibration {number}"),
            correction: prysm_contract::ColorCorrection::default(),
        }
    }

    pub fn active(&self) -> bool {
        matches!(self.outcome, Outcome::Active)
    }

    pub fn started(&self) -> bool {
        self.started
    }

    pub fn startup_cancel(&self) -> CancellationToken {
        self.startup_cancel.clone()
    }

    pub fn cancel_startup(&self) {
        self.startup_cancel.cancel();
    }

    pub fn finish(&mut self, message: &str) {
        self.cancel_startup();
        self.outcome = if matches!(self.outcome, Outcome::Saved(_)) {
            Outcome::Saved(message.into())
        } else {
            Outcome::Discarded(message.into())
        };
    }

    pub fn saved(&mut self, profile: CalibrationProfile) {
        self.draft = profile;
        self.outcome = Outcome::Saved("Calibration saved and active.".into());
        self.cancel_startup();
    }

    fn authorize(&self, token: OperationId, phone_only: bool) -> Result<(), CommandError> {
        if self.phone == Some(token) || (!phone_only && self.tv == token) {
            Ok(())
        } else {
            Err(CommandError::Conflict(
                "calibration session is no longer available",
            ))
        }
    }

    pub fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.tv_seen) >= RECONNECT
            || if self.phone.is_some() {
                now.duration_since(self.phone_seen) >= RECONNECT
            } else {
                now.duration_since(self.created) >= PAIRING
            }
    }

    fn pair(&mut self, pairing: OperationId, now: Instant) -> Result<OperationId, CommandError> {
        if !self.active() || self.expired(now) || pairing != self.pairing || self.phone.is_some() {
            return Err(CommandError::Conflict(
                "pairing link expired or already used; start again on the TV",
            ));
        }
        let token = OperationId::new();
        self.phone = Some(token);
        self.phone_seen = now;
        Ok(token)
    }

    fn compare(&mut self, held: bool, now: Instant) {
        self.compare_until = if held {
            now + Duration::from_secs(2)
        } else {
            now
        };
    }

    fn select_profile(
        &mut self,
        id: Option<OperationId>,
        profiles: &CalibrationProfiles,
        now: Instant,
    ) -> Result<(), CommandError> {
        let draft = match id {
            Some(id) => profiles
                .profiles
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .ok_or(CommandError::Conflict("profile no longer exists"))?,
            None => Self::new_profile(profiles),
        };
        self.draft = draft;
        self.replacing = id.is_some();
        self.phone_seen = now;
        self.step = CalibrationStep::Response;
        self.sequence = self.sequence.saturating_add(1);
        self.compare(false, now);
        Ok(())
    }

    fn prepare_save(
        &self,
        name: String,
        mut settings: SettingsView,
        startup_pending: bool,
    ) -> Result<Effect, CommandError> {
        if self.step != CalibrationStep::Review || !self.started || startup_pending {
            return Err(CommandError::Conflict(
                "review the calibration before saving",
            ));
        }
        prysm_contract::validate_name(&name).map_err(CommandError::Invalid)?;
        let mut profile = self.draft.clone();
        profile.name = name;
        let profiles = &mut settings.settings.calibration;
        if self.replacing {
            *profiles
                .profiles
                .iter_mut()
                .find(|p| p.id == profile.id)
                .ok_or(CommandError::Conflict("profile no longer exists"))? = profile.clone();
        } else {
            profiles.profiles.push(profile.clone());
        }
        profiles.active = Some(profile.id);
        Ok(Effect::Save {
            update: Box::new(UpdateSettings {
                expected_revision: settings.revision,
                settings: settings.settings,
            }),
            profile,
        })
    }

    pub fn transition(
        &mut self,
        request: CalibrationRequest,
        settings: &SettingsView,
        startup_pending: bool,
        now: Instant,
    ) -> Result<(OperationId, Effect), CommandError> {
        let token = match &request {
            CalibrationRequest::Pair { pairing } => {
                return Ok((self.pair(*pairing, now)?, Effect::None));
            }
            CalibrationRequest::Poll { token, .. }
            | CalibrationRequest::Select { token, .. }
            | CalibrationRequest::Update { token, .. }
            | CalibrationRequest::Save { token, .. }
            | CalibrationRequest::Cancel { token } => *token,
            CalibrationRequest::Begin => return Err(CommandError::Conflict(LOCKED)),
        };
        self.authorize(
            token,
            matches!(
                request,
                CalibrationRequest::Select { .. }
                    | CalibrationRequest::Update { .. }
                    | CalibrationRequest::Save { .. }
            ),
        )?;
        if !self.active() {
            return Ok((token, Effect::None));
        }
        if self.expired(now) {
            return Ok((
                token,
                Effect::End("Calibration ended after the connection was lost."),
            ));
        }
        let effect = match request {
            CalibrationRequest::Poll {
                compare, playing, ..
            } => {
                if token == self.tv {
                    self.tv_seen = now;
                } else {
                    self.phone_seen = now;
                    self.compare(compare, now);
                }
                if token == self.tv && playing && self.phone.is_some() && !self.started {
                    self.started = true;
                    Effect::Start
                } else {
                    Effect::None
                }
            }
            CalibrationRequest::Select { profile, .. } => {
                self.select_profile(profile, &settings.settings.calibration, now)?;
                Effect::None
            }
            CalibrationRequest::Update {
                sequence,
                correction,
                step,
                shade,
                paused,
                compare,
                ..
            } => {
                if shade > 5
                    || (matches!(step, CalibrationStep::Intensity | CalibrationStep::Mixed)
                        && shade > 2)
                {
                    return Err(CommandError::Invalid(anyhow::anyhow!(
                        "invalid calibration pattern"
                    )));
                }
                self.phone_seen = now;
                if sequence > self.sequence {
                    self.sequence = sequence;
                    self.draft.correction = correction;
                    self.step = step;
                    self.shade = shade;
                    self.paused = paused;
                    self.compare(compare, now);
                }
                Effect::None
            }
            CalibrationRequest::Save { name, .. } => {
                self.prepare_save(name, settings.clone(), startup_pending)?
            }
            CalibrationRequest::Cancel { .. } => Effect::End(if token == self.tv {
                "Calibration ended on the TV. Unsaved changes discarded."
            } else {
                "Calibration cancelled. Unsaved changes discarded."
            }),
            CalibrationRequest::Begin | CalibrationRequest::Pair { .. } => {
                unreachable!("handled above")
            }
        };
        Ok((token, effect))
    }

    pub fn tv_view(&self, now: Instant) -> CalibrationView {
        self.view(self.tv, now)
    }

    pub fn view(&self, token: OperationId, now: Instant) -> CalibrationView {
        CalibrationView {
            saved: matches!(self.outcome, Outcome::Saved(_)),
            token,
            pairing: (token == self.tv && self.phone.is_none() && self.active())
                .then_some(self.pairing),
            connected: self.phone.is_some(),
            draft: self.draft.clone(),
            step: self.step,
            shade: self.shade,
            paused: self.paused,
            compare: self.active() && now < self.compare_until,
            sequence: self.sequence,
            ended: match &self.outcome {
                Outcome::Active => None,
                Outcome::Saved(message) | Outcome::Discarded(message) => Some(message.clone()),
            },
        }
    }

    pub fn pending_config(&self, config: Config, now: Instant) -> Option<Config> {
        if !self.active() || !self.started {
            return None;
        }
        let effective = self.effective(config, now);
        (self.applied.as_ref() != Some(&effective)).then_some(effective)
    }

    pub fn mark_applied(&mut self, config: Config) {
        self.applied = Some(config);
    }

    fn effective(&self, mut config: Config, now: Instant) -> Config {
        config.correction = if now < self.compare_until {
            prysm_contract::ColorCorrection::default()
        } else {
            self.draft.correction
        };
        if !matches!(self.step, CalibrationStep::Review | CalibrationStep::Mixed) {
            config.processing.brightness_percent = 70.try_into().unwrap();
            config.processing.shadow_lift_percent = 0.try_into().unwrap();
            config.processing.smoothing_seconds = 0.0.try_into().unwrap();
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> SettingsView {
        let directory = tempfile::tempdir().unwrap();
        crate::test_support::store(&directory, &crate::test_support::config()).view()
    }

    fn poll(token: OperationId, playing: bool, compare: bool) -> CalibrationRequest {
        CalibrationRequest::Poll {
            token,
            playing,
            compare,
        }
    }

    fn adjustment(token: OperationId, sequence: u32, step: CalibrationStep) -> CalibrationRequest {
        CalibrationRequest::Update {
            token,
            sequence,
            step,
            shade: 0,
            paused: false,
            compare: true,
            correction: prysm_contract::ColorCorrection {
                warmth: 50.try_into().unwrap(),
                ..prysm_contract::ColorCorrection::default()
            },
        }
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "follow one authorized session through ordered updates, comparison and cancellation"
    )]
    fn authorization_ordering_and_comparison_are_session_rules() {
        let now = Instant::now();
        let settings = settings();
        let config = settings.settings.config(settings.revision).unwrap();
        let mut session = Session::new(&settings, now);
        let tv = session.tv_view(now);
        assert!(
            session
                .transition(
                    poll(OperationId::new(), false, false),
                    &settings,
                    false,
                    now
                )
                .is_err()
        );
        let pair = CalibrationRequest::Pair {
            pairing: tv.pairing.unwrap(),
        };
        let (phone, _) = session
            .transition(pair.clone(), &settings, false, now)
            .unwrap();
        assert!(session.transition(pair, &settings, false, now).is_err());
        assert!(
            session
                .transition(
                    adjustment(tv.token, 1, CalibrationStep::WhiteBalance),
                    &settings,
                    false,
                    now
                )
                .is_err()
        );
        assert!(matches!(
            session
                .transition(poll(tv.token, true, false), &settings, false, now)
                .unwrap()
                .1,
            Effect::Start
        ));
        assert!(matches!(
            session
                .transition(poll(tv.token, true, false), &settings, false, now)
                .unwrap()
                .1,
            Effect::None
        ));
        session
            .transition(
                adjustment(phone, 2, CalibrationStep::WhiteBalance),
                &settings,
                false,
                now,
            )
            .unwrap();
        session
            .transition(
                adjustment(phone, 1, CalibrationStep::Review),
                &settings,
                false,
                now,
            )
            .unwrap();
        assert_eq!(session.view(phone, now).step, CalibrationStep::WhiteBalance);
        let reference = session.pending_config(config.clone(), now).unwrap();
        assert_eq!(
            reference.correction,
            prysm_contract::ColorCorrection::default()
        );
        assert_eq!(reference.processing.brightness_percent.get(), 70);
        assert_eq!(reference.processing.smoothing_seconds.get(), 0.0);
        assert_eq!(reference.processing.shadow_lift_percent.get(), 0);
        session.mark_applied(reference);
        assert!(session.pending_config(config.clone(), now).is_none());
        let later = now + Duration::from_secs(3);
        assert_eq!(
            session
                .pending_config(config.clone(), later)
                .unwrap()
                .correction
                .warmth
                .get(),
            50
        );
        session
            .transition(
                adjustment(phone, 3, CalibrationStep::Review),
                &settings,
                false,
                later,
            )
            .unwrap();
        assert_eq!(
            session
                .pending_config(config.clone(), later)
                .unwrap()
                .processing,
            config.processing
        );
        let mut invalid = adjustment(phone, 4, CalibrationStep::Colors);
        if let CalibrationRequest::Update { shade, .. } = &mut invalid {
            *shade = 6;
        }
        assert!(
            session
                .transition(invalid, &settings, false, later)
                .is_err()
        );
        assert_eq!(session.view(phone, later).sequence, 3);
        session.finish("Cancelled");
        assert!(session.startup_cancel().is_cancelled());
        assert!(session.pending_config(config, later).is_none());
        session
            .transition(
                adjustment(phone, 5, CalibrationStep::Colors),
                &settings,
                false,
                later,
            )
            .unwrap();
        let ended = session.view(phone, later);
        assert_eq!(ended.sequence, 3);
        assert!(!ended.saved);
        assert!(!ended.compare);
    }

    #[test]
    fn profile_selection_and_save_wait_for_review_and_persistence() {
        let now = Instant::now();
        let mut settings = settings();
        let profile = CalibrationProfile {
            id: OperationId::new(),
            name: "Movie".into(),
            correction: prysm_contract::ColorCorrection::default(),
        };
        settings.settings.calibration.profiles.push(profile.clone());
        let mut session = Session::new(&settings, now);
        let tv = session.tv_view(now);
        let (phone, _) = session
            .transition(
                CalibrationRequest::Pair {
                    pairing: tv.pairing.unwrap(),
                },
                &settings,
                false,
                now,
            )
            .unwrap();
        session
            .transition(
                CalibrationRequest::Select {
                    token: phone,
                    profile: Some(profile.id),
                },
                &settings,
                false,
                now,
            )
            .unwrap();
        let save = CalibrationRequest::Save {
            token: phone,
            name: "Movie updated".into(),
        };
        assert!(
            session
                .transition(save.clone(), &settings, false, now)
                .is_err()
        );
        session
            .transition(poll(tv.token, true, false), &settings, false, now)
            .unwrap();
        session
            .transition(
                adjustment(phone, 2, CalibrationStep::Review),
                &settings,
                false,
                now,
            )
            .unwrap();
        assert!(
            session
                .transition(save.clone(), &settings, true, now)
                .is_err()
        );
        let (
            _,
            Effect::Save {
                update,
                profile: saved,
            },
        ) = session.transition(save, &settings, false, now).unwrap()
        else {
            panic!("save must prepare a settings update")
        };
        assert_eq!(update.expected_revision, settings.revision);
        assert_eq!(update.settings.calibration.profiles, vec![saved.clone()]);
        assert_eq!(update.settings.calibration.active, Some(profile.id));
        assert_eq!(update.settings.processing, settings.settings.processing);
        assert!(
            session.active(),
            "preparing a write must not finish the session"
        );
        assert!(!session.view(phone, now).saved);
        session.saved(saved);
        session.finish("Saved; runtime cleanup failed");
        assert!(!session.active());
        assert!(
            session.view(phone, now).saved,
            "a cleanup error cannot undo persistence"
        );
        assert!(session.view(phone, now).ended.is_some());
    }

    #[test]
    fn pairing_and_reconnection_expire_without_extending_old_capabilities() {
        let now = Instant::now();
        let settings = settings();
        let mut session = Session::new(&settings, now);
        let tv = session.tv_view(now);
        for seconds in [29, 58, 87, 116] {
            session
                .transition(
                    poll(tv.token, false, false),
                    &settings,
                    false,
                    now + Duration::from_secs(seconds),
                )
                .unwrap();
        }
        assert!(
            session
                .transition(
                    CalibrationRequest::Pair {
                        pairing: tv.pairing.unwrap()
                    },
                    &settings,
                    false,
                    now + PAIRING
                )
                .is_err()
        );
        let mut session = Session::new(&settings, now);
        let tv = session.tv_view(now);
        let (phone, _) = session
            .transition(
                CalibrationRequest::Pair {
                    pairing: tv.pairing.unwrap(),
                },
                &settings,
                false,
                now,
            )
            .unwrap();
        for token in [tv.token, phone] {
            session
                .transition(
                    poll(token, false, false),
                    &settings,
                    false,
                    now + Duration::from_secs(29),
                )
                .unwrap();
        }
        assert!(!session.expired(now + Duration::from_secs(58)));
        let expired = now + Duration::from_secs(59);
        assert!(matches!(
            session
                .transition(poll(phone, false, false), &settings, false, expired)
                .unwrap()
                .1,
            Effect::End(_)
        ));
        assert!(session.expired(expired));
    }
}
