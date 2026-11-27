//! Settings changes and the restarts they cause, revisions and validation.
use crate::common::*;
use prysm_ports::{
    AdapterSettings, Error, LightingTarget, Revision, SessionState, Settings, SettingsStore,
    Snapshot,
};
use prysm_runtime::Engine;
use prysm_runtime::fakes::{CallLog, FakeOutputSettings, FakeSourceSettings, MemoryStore};

#[tokio::test]
async fn processing_change_keeps_source_and_output_open() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, store) = start(&log, named("a", "a"), DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:a", 2).await);

        let mut changed = named("a", "a");
        changed.processing.brightness_percent = 40;
        let initial = revision(&handle);
        let replaced = handle
            .replace_settings(initial, changed.clone())
            .await
            .unwrap();
        assert_eq!(replaced, initial.next());
        assert_eq!(revision(&handle), replaced);
        let writes = log.count("output.write:a");
        assert!(log.wait_for("output.write:a", writes + 3).await);

        assert_eq!(log.count("source.open:a"), 1);
        assert_eq!(log.count("output.open:a"), 1);
        assert_eq!(log.count("source.close:a"), 0);
        assert_eq!(store.saved(), Some(changed.clone()));
        assert_eq!(handle.snapshot().borrow().settings, changed);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn adapter_change_restarts_a_running_session() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:a", 2).await);

        let replaced = handle
            .replace_settings(revision(&handle), named("b", "a"))
            .await
            .unwrap();
        assert!(log.wait_for("output.open:a", 2).await);
        assert!(before(&log, "source.close:a", "output.idle:a:Native"));
        assert!(before(&log, "output.idle:a:Native", "source.open:b"));
        assert!(log.wait_for("output.open:a", 2).await);
        snapshot_until(&handle, |s| matches!(s.session, SessionState::ScreenSync)).await;

        handle
            .replace_settings(replaced, named("b", "c"))
            .await
            .unwrap();
        assert!(log.wait_for("output.write:c", 1).await);
        assert_eq!(log.count("source.open:b"), 2);
        assert!(log.last_position("source.close:b") < log.last_position("source.open:b"));
        engine.shutdown().await.unwrap();
        assert_eq!(log.count("output.idle:c:Off"), 1);
    })
    .await;
}

#[tokio::test]
async fn adapter_change_while_off_opens_nothing() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        handle
            .replace_settings(revision(&handle), named("b", "b"))
            .await
            .unwrap();
        assert!(log.events().is_empty());
        engine.shutdown().await.unwrap();
        assert!(log.events().is_empty());
    })
    .await;
}

#[tokio::test]
async fn invalid_or_stale_settings_are_rejected_before_persistence() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let initial = named("a", "a");
        let (handle, engine, store) = start(&log, initial.clone(), DEADLINE).await;
        let current = revision(&handle);

        let invalid = settings(
            FakeSourceSettings {
                invalid: true,
                ..Default::default()
            },
            FakeOutputSettings::default(),
        );
        let error = handle.replace_settings(current, invalid).await.unwrap_err();
        assert!(matches!(error, Error::Invalid(ref e) if e.field == "capture.invalid"));

        let mut unknown = named("a", "a");
        unknown.output = Some(AdapterSettings::new("nope", &FakeOutputSettings::default()));
        let error = handle.replace_settings(current, unknown).await.unwrap_err();
        assert!(matches!(error, Error::Invalid(ref e) if e.field == "output.kind"));

        let mut out_of_range = named("a", "a");
        out_of_range.processing.brightness_percent = 101;
        assert!(
            handle
                .replace_settings(current, out_of_range)
                .await
                .is_err()
        );

        let stale = Revision(current.0.wrapping_sub(1));
        let error = handle
            .replace_settings(stale, named("b", "b"))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::StaleRevision { expected, current: now } if expected == stale && now == current
        ));

        assert_eq!(store.saved(), Some(initial));
        assert_eq!(revision(&handle), current);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn each_engine_starts_at_its_own_revision_so_old_revisions_are_stale() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let store = MemoryStore::with(named("a", "a"));
        let (first, engine) = Engine::start(registry(&log), Box::new(store.clone()), DEADLINE)
            .await
            .unwrap();
        let before_restart = revision(&first);
        engine.shutdown().await.unwrap();

        let (second, engine) = Engine::start(registry(&log), Box::new(store.clone()), DEADLINE)
            .await
            .unwrap();
        let current = revision(&second);
        assert_ne!(before_restart, current);
        assert!(current.0 < 1 << 52, "{current} must stay an exact JSON number");
        let error = second
            .replace_settings(before_restart, named("b", "b"))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::StaleRevision { expected, current: now } if expected == before_restart && now == current
        ));
        assert_eq!(store.saved(), Some(named("a", "a")));
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn output_change_while_off_neither_opens_nor_powers_the_new_output() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, store) = start(&log, named("a", "a"), DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:a", 1).await);
        snapshot_until(&handle, |s| matches!(s.session, SessionState::ScreenSync)).await;
        handle.set_target(LightingTarget::Off).await.unwrap();
        snapshot_until(&handle, |s| {
            s.control.power == Some(prysm_ports::Power::Off)
        })
        .await;
        let events = log.events();

        handle
            .replace_settings(revision(&handle), named("a", "b"))
            .await
            .unwrap();
        assert_eq!(store.saved(), Some(named("a", "b")));
        assert!(matches!(
            handle.snapshot().borrow().session,
            SessionState::Stopped { .. }
        ));
        engine.shutdown().await.unwrap();
        assert_eq!(log.events(), events);
    })
    .await;
}

#[tokio::test]
async fn concurrent_replacements_of_one_revision_admit_exactly_one() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, store) = start(&log, named("a", "a"), DEADLINE).await;
        let current = revision(&handle);
        let (b, c) = tokio::join!(
            handle.replace_settings(current, named("b", "b")),
            handle.replace_settings(current, named("c", "c")),
        );

        let (winner, loser) = match (b, c) {
            (Ok(revision), Err(error)) => ((revision, named("b", "b")), error),
            (Err(error), Ok(revision)) => ((revision, named("c", "c")), error),
            other => panic!("expected exactly one winner, got {other:?}"),
        };
        assert_eq!(winner.0, current.next());
        assert!(matches!(
            loser,
            Error::StaleRevision { expected, current: now } if expected == current && now == winner.0
        ));
        assert_eq!(store.saved(), Some(winner.1.clone()));
        assert_eq!(handle.snapshot().borrow().settings, winner.1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn invalid_loaded_settings_stay_reported_after_a_successful_command() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let invalid = settings(
            FakeSourceSettings {
                invalid: true,
                ..Default::default()
            },
            FakeOutputSettings::default(),
        );
        let (handle, engine, _) = start(&log, invalid, DEADLINE).await;
        let invalid_capture = |s: &Snapshot| {
            matches!(&s.session.failure(), Some(Error::Invalid(e)) if e.field == "capture.invalid")
        };
        assert!(invalid_capture(&handle.snapshot().borrow()));

        handle
            .set_target(LightingTarget::Effects)
            .await
            .unwrap();
        snapshot_until(&handle, |s| matches!(s.session, SessionState::Effects { .. })).await;
        assert!(invalid_capture(&handle.snapshot().borrow()));

        handle
            .replace_settings(revision(&handle), named("a", "a"))
            .await
            .unwrap();
        assert!(handle.snapshot().borrow().session.failure().is_none());
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn an_unreadable_store_starts_unconfigured_and_reports_it_until_a_save() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let saved = MemoryStore::new();
        let (handle, engine) = Engine::start(
            registry(&log),
            Box::new(Unreadable(saved.clone())),
            DEADLINE,
        )
        .await
        .unwrap();
        let unreadable = |s: &Snapshot| {
            matches!(&s.session.failure(), Some(Error::UnusableSettings(stored))
                if stored.to_string() == "cannot parse settings file")
        };
        assert!(unreadable(&handle.snapshot().borrow()));
        assert_eq!(handle.snapshot().borrow().settings, Settings::default());

        handle.set_target(LightingTarget::Off).await.unwrap();
        assert!(
            unreadable(&handle.snapshot().borrow()),
            "a command keeps it"
        );
        for target in [LightingTarget::ScreenSync, LightingTarget::Effects] {
            handle.set_target(target).await.unwrap();
            assert!(unreadable(&handle.snapshot().borrow()), "{target:?}");
        }
        handle.set_target(LightingTarget::Off).await.unwrap();
        assert!(unreadable(&handle.snapshot().borrow()));

        handle
            .replace_settings(revision(&handle), named("a", "a"))
            .await
            .unwrap();
        assert!(handle.snapshot().borrow().session.failure().is_none());
        assert_eq!(saved.saved(), Some(named("a", "a")));
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_refused_save_installs_nothing() {
    within_async(LIMIT, async {
        for unreadable in [false, true] {
            let log = CallLog::new();
            let store = Box::new(Refusing { unreadable });
            let (handle, engine) = Engine::start(registry(&log), store, DEADLINE)
                .await
                .unwrap();
            let before = revision(&handle);
            let error = handle
                .replace_settings(before, named("a", "a"))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "cannot save settings file");
            assert_eq!(revision(&handle), before);
            let snapshot = handle.snapshot().borrow().clone();
            assert_eq!(snapshot.settings, Settings::default());
            assert_eq!(
                matches!(snapshot.session.failure(), Some(Error::UnusableSettings(_))),
                unreadable,
                "{:?}",
                snapshot.session.failure()
            );
            engine.shutdown().await.unwrap();
        }
    })
    .await;
}

/// Loads fail, like a settings file that does not decode; saves are kept.
struct Unreadable(MemoryStore);

impl SettingsStore for Unreadable {
    fn load(&mut self) -> prysm_ports::Result<Option<Settings>> {
        Err(Error::other("cannot parse settings file"))
    }

    fn save(&mut self, settings: &Settings) -> prysm_ports::Result<()> {
        self.0.save(settings)
    }
}

/// Saves fail, as a store does whose stored settings stay as they were;
/// loads fail too when `unreadable`.
struct Refusing {
    unreadable: bool,
}

impl SettingsStore for Refusing {
    fn load(&mut self) -> prysm_ports::Result<Option<Settings>> {
        if self.unreadable {
            return Err(Error::other("cannot parse settings file"));
        }
        Ok(None)
    }

    fn save(&mut self, _: &Settings) -> prysm_ports::Result<()> {
        Err(Error::other("cannot save settings file"))
    }
}
