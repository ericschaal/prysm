use super::*;
use crate::disk::{Stage, Store};
use prysm_contract::{
    FailureKind,
    calibration::{CalibrationProfile, ProfileId},
};
use prysm_core::{ColorCorrection, SmoothingSeconds};
use std::{fs, future::pending, num::NonZeroU64, sync::mpsc, time::Duration};

type Value = Settings<u32, u32>;
fn value(capture: u32) -> Value {
    Value {
        capture: Some(capture),
        output: Some(2),
        processing: Default::default(),
        profiles: vec![],
        active_profile: None,
    }
}
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("file worker stalled")
}
async fn close(file: FileSettings<u32, u32>) -> Result<OwnerStopped, ControlError> {
    file.shutdown.request();
    within(file.owner.run(pending())).await
}
fn gated(
    path: PathBuf,
    stage: Stage,
    fail: bool,
) -> (
    FileSettings<u32, u32>,
    oneshot::Receiver<()>,
    mpsc::SyncSender<()>,
) {
    let (entered, started) = oneshot::channel();
    let (release, released) = mpsc::sync_channel(1);
    let mut entered = Some(entered);
    let mut store = Store::new(path).unwrap();
    store.checkpoint = Some(Box::new(move |at| {
        if at == stage
            && let Some(entered) = entered.take()
        {
            entered.send(()).unwrap();
            released.recv().unwrap();
            if fail {
                return Err(std::io::Error::other("injected commit failure"));
            }
        }
        Ok(())
    }));
    (FileSettings::open(store).unwrap(), started, release)
}

#[tokio::test]
async fn missing_file_round_trip_and_restart_preserve_complete_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let file = FileSettings::<u32, u32>::new(path.clone()).unwrap();
    assert!(file.repository.load().await.unwrap().is_none());
    assert!(!path.exists());
    let mut saved = value(7);
    saved.processing.smoothing_seconds = SmoothingSeconds(0.31415927);
    saved.processing.correction = ColorCorrection::new(37, -81, 59)
        .unwrap()
        .with_chroma(-32, [1, 2, 3, -4, -5, -6])
        .unwrap();
    saved.profiles.push(CalibrationProfile {
        id: ProfileId(NonZeroU64::MAX),
        name: "Calibrated".into(),
        correction: saved.processing.correction,
    });
    saved.active_profile = Some(ProfileId(NonZeroU64::MAX));
    file.repository.save(saved.clone()).await.unwrap();
    assert_eq!(file.repository.load().await.unwrap(), Some(saved.clone()));
    close(file).await.unwrap();
    let restarted = FileSettings::new(path).unwrap();
    assert_eq!(restarted.repository.load().await.unwrap(), Some(saved));
    close(restarted).await.unwrap();
}

#[tokio::test]
async fn malformed_or_invalid_file_is_never_replaced_with_defaults() {
    for bytes in [
        "{broken",
        r#"{"processing":{"smoothing_seconds":1e400}}"#,
        r#"{"active_profile":1}"#,
        r#"{"unknown":true}"#,
        r#"{"profiles":[{"id":1,"name":"bad","correction":{"gains":[0,1,1],"tone":1,"intensity":0,"hue":[0,0,0,0,0,0]}}]}"#,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, bytes).unwrap();
        let file = FileSettings::<u32, u32>::new(path.clone()).unwrap();
        assert_eq!(
            file.repository.load().await.unwrap_err().kind(),
            FailureKind::Invalid
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
        assert_eq!(close(file).await.unwrap_err().kind(), FailureKind::Invalid);
    }
}

#[tokio::test]
async fn invalid_save_preserves_old_file_and_allows_a_valid_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let file = FileSettings::<u32, u32>::new(path.clone()).unwrap();
    file.repository.save(value(1)).await.unwrap();
    let old = fs::read(&path).unwrap();
    let mut invalid = value(9);
    invalid.processing.smoothing_seconds = SmoothingSeconds(f32::NAN);
    assert_eq!(
        file.repository.save(invalid).await.unwrap_err().kind(),
        FailureKind::Invalid
    );
    assert_eq!(fs::read(&path).unwrap(), old);
    file.repository.save(value(3)).await.unwrap();
    assert_eq!(file.repository.load().await.unwrap(), Some(value(3)));
    // A successful retry cannot hide a prior failure from the host.
    assert_eq!(close(file).await.unwrap_err().kind(), FailureKind::Invalid);
}

#[tokio::test]
async fn precommit_failure_preserves_destination_and_removes_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let original = format::encode(value(1)).unwrap();
    fs::write(&path, &original).unwrap();
    let (file, entered, release) = gated(path.clone(), Stage::BeforeRename, true);
    let repository = file.repository.clone();
    let saving = tokio::spawn(async move { repository.save(value(2)).await });
    within(entered).await.unwrap();
    release.send(()).unwrap();
    assert_eq!(
        within(saving).await.unwrap().unwrap_err().kind(),
        FailureKind::Adapter
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    close(file).await.unwrap_err();
}

#[tokio::test]
async fn post_rename_failure_is_uncertain_fences_writes_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, format::encode(value(1)).unwrap()).unwrap();
    let (file, entered, release) = gated(path.clone(), Stage::AfterRename, true);
    let repository = file.repository.clone();
    let saving = tokio::spawn(async move { repository.save(value(2)).await });
    within(entered).await.unwrap();
    assert_eq!(
        format::decode::<u32, u32>(&fs::read(&path).unwrap()).unwrap(),
        value(2)
    );
    release.send(()).unwrap();
    assert_eq!(
        within(saving).await.unwrap().unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
    assert_eq!(
        file.repository.save(value(3)).await.unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
    assert_eq!(
        close(file).await.unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
    let restarted = FileSettings::new(path).unwrap();
    assert_eq!(restarted.repository.load().await.unwrap(), Some(value(2)));
    close(restarted).await.unwrap();
}

#[tokio::test]
async fn cancelled_waiter_and_shutdown_retain_commit_and_its_failure() {
    for fail in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, format::encode(value(1)).unwrap()).unwrap();
        let (file, entered, release) = gated(path.clone(), Stage::AfterRename, fail);
        let repository = file.repository.clone();
        let saving = tokio::spawn(async move { repository.save(value(2)).await });
        within(entered).await.unwrap();
        saving.abort();
        assert!(within(saving).await.unwrap_err().is_cancelled());
        file.shutdown.request();
        assert_eq!(
            file.repository.save(value(3)).await.unwrap_err().kind(),
            FailureKind::Unavailable
        );
        let owner = tokio::spawn(file.owner.run(pending()));
        tokio::task::yield_now().await;
        assert!(!owner.is_finished());
        release.send(()).unwrap();
        let result = within(owner).await.unwrap();
        if fail {
            assert_eq!(result.unwrap_err().kind(), FailureKind::RecoveryRequired);
        } else {
            result.unwrap();
        }
        assert_eq!(
            format::decode::<u32, u32>(&fs::read(path).unwrap()).unwrap(),
            value(2)
        );
    }
}

#[tokio::test]
async fn rename_failure_and_missing_parent_do_not_destroy_existing_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("directory.json");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("keep"), "important").unwrap();
    let file = FileSettings::<u32, u32>::new(path.clone()).unwrap();
    assert_eq!(
        file.repository.save(value(2)).await.unwrap_err().kind(),
        FailureKind::Adapter
    );
    assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "important");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    close(file).await.unwrap_err();
    let file = FileSettings::<u32, u32>::new(dir.path().join("missing/settings.json")).unwrap();
    assert_eq!(
        file.repository.save(value(2)).await.unwrap_err().kind(),
        FailureKind::Adapter
    );
    close(file).await.unwrap_err();
}

#[tokio::test]
async fn shutdown_drains_the_bounded_accepted_queue_without_admitting_more() {
    use std::task::{Context, Poll, Waker};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let (file, entered, release) = gated(path.clone(), Stage::BeforeRename, false);
    let repository = file.repository.clone();
    let first = tokio::spawn(async move { repository.save(value(1)).await });
    within(entered).await.unwrap();
    let mut second = Box::pin(file.repository.save(value(2)));
    assert!(matches!(
        second
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    assert_eq!(
        file.repository.save(value(3)).await.unwrap_err().kind(),
        FailureKind::Conflict
    );
    file.shutdown.request();
    assert_eq!(
        file.repository.save(value(4)).await.unwrap_err().kind(),
        FailureKind::Unavailable
    );
    release.send(()).unwrap();
    within(first).await.unwrap().unwrap();
    within(second).await.unwrap();
    assert_eq!(
        format::decode::<u32, u32>(&fs::read(path).unwrap()).unwrap(),
        value(2)
    );
    close(file).await.unwrap();
}

#[tokio::test]
async fn uncertainty_rejects_a_save_already_queued_without_writing_it() {
    use std::task::{Context, Poll, Waker};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let (file, entered, release) = gated(path.clone(), Stage::AfterRename, true);
    let repository = file.repository.clone();
    let first = tokio::spawn(async move { repository.save(value(1)).await });
    within(entered).await.unwrap();
    let mut queued = Box::pin(file.repository.save(value(2)));
    assert!(matches!(
        queued
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    release.send(()).unwrap();
    assert_eq!(
        within(first).await.unwrap().unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
    assert_eq!(
        within(queued).await.unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
    assert_eq!(
        format::decode::<u32, u32>(&fs::read(path).unwrap()).unwrap(),
        value(1)
    );
    assert_eq!(
        close(file).await.unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
}

#[tokio::test]
async fn worker_panic_after_rename_reports_uncertainty_to_caller_and_host() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut store = Store::new(path.clone()).unwrap();
    store.checkpoint = Some(Box::new(|stage| {
        assert!(stage != Stage::AfterRename, "worker panic after commit");
        Ok(())
    }));
    let file = FileSettings::<u32, u32>::open(store).unwrap();
    assert_eq!(
        within(file.repository.save(value(2)))
            .await
            .unwrap_err()
            .kind(),
        FailureKind::RecoveryRequired
    );
    assert_eq!(
        close(file).await.unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
    assert_eq!(
        format::decode::<u32, u32>(&fs::read(path).unwrap()).unwrap(),
        value(2)
    );
}
