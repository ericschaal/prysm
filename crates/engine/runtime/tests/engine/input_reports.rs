//! What the snapshot reports about a running session: rates, latency, issues, the slow step.
use crate::common::*;
use prysm_ports::{
    Behind, DeviceBrightness, DeviceReport, Latency, LightingTarget, OutputIssue, Picture,
    SessionState,
};
use prysm_runtime::EngineHandle;
use prysm_runtime::fakes::{CallLog, FakeOutputSettings, FakeSourceSettings};
use std::time::Duration;

#[tokio::test]
async fn input_reports_frame_rate_and_quiet_sources_about_once_a_second() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let quiet = settings(
            FakeSourceSettings {
                name: "a".into(),
                quiet_after: Some(150),
                ..Default::default()
            },
            FakeOutputSettings::default(),
        );
        let (handle, engine, _) = start(&log, quiet, DEADLINE).await;
        assert!(handle.snapshot().borrow().input.is_none(), "no session");

        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        let started = tokio::time::Instant::now();
        let waiting = handle.snapshot().borrow().input.unwrap();
        assert_eq!(waiting.picture, Picture::Quiet, "nothing measured yet");

        // The fake delivers a frame about every 2 ms.
        let receiving = snapshot_until(&handle, |s| arriving(s.input)).await;
        assert!(
            started.elapsed() >= Duration::from_millis(900),
            "sampled per second"
        );
        // The exact rate depends on the machine's load; receiving means at
        // least one frame in the window.
        let fps = receiving.input.unwrap().fps;
        assert!(fps > 0.0, "{fps}");

        // After 150 frames it goes quiet.
        let quiet = snapshot_until(&handle, |s| !arriving(s.input)).await;
        assert!(quiet.input.unwrap().fps.abs() < f32::EPSILON);
        assert_eq!(
            quiet.input.unwrap().picture,
            Picture::Quiet,
            "quiet, for no reason given"
        );
        assert!(
            matches!(quiet.session, SessionState::ScreenSync),
            "{}",
            quiet.session
        );

        handle.set_target(LightingTarget::Off).await.unwrap();
        assert!(handle.snapshot().borrow().input.is_none());
        engine.shutdown().await.unwrap();
    })
    .await;
}

fn arriving(input: Option<prysm_ports::Input>) -> bool {
    input.is_some_and(|i| i.picture.is_arriving())
}

/// A source whose device refuses to share its video: the input says so
/// while no frame arrives, and frames withdraw it.
#[tokio::test]
async fn input_reports_a_refused_video_until_frames_come_again() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let refused = settings(
            FakeSourceSettings {
                name: "a".into(),
                quiet_after: Some(0),
                quiet_for_ms: 1500,
                refused: true,
                ..Default::default()
            },
            FakeOutputSettings::default(),
        );
        let (handle, engine, _) = start(&log, refused, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        let waiting = snapshot_until(&handle, |s| {
            s.input.is_some_and(|i| i.picture == Picture::Refused)
        })
        .await;
        assert!(matches!(waiting.session, SessionState::ScreenSync));

        snapshot_until(&handle, |s| arriving(s.input)).await;
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// A source that says when it captured each frame: the span starts there.
#[tokio::test]
async fn latency_starts_at_capture_when_every_frame_has_a_capture_time() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let captured = settings(
            FakeSourceSettings {
                name: "a".into(),
                captured_ago_ms: Some(300),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, captured, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        let measured =
            snapshot_until(&handle, |s| s.input.is_some_and(|i| i.latency.is_some())).await;
        let input = measured.input.unwrap();
        let Latency {
            capture_ms,
            output_ms,
        } = input.latency.unwrap();
        // Captured 300 ms before the fake hands it over: that part is before
        // Prysm; the rest is Prysm's own. The upper bounds only leave room
        // for a slow machine.
        let capture_ms = capture_ms.expect("every frame had a capture time");
        assert!((300.0..1000.0).contains(&capture_ms), "{capture_ms}");
        assert!(output_ms < 500.0, "{output_ms}");
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// What reaches the lights: the rate of frames written, and per frame the
/// span from Prysm receiving it to the write for it returning (here a slow
/// device of 20 ms), as a mean over the sample window. Frames whose write
/// fails do not count.
#[tokio::test]
async fn input_reports_frames_sent_and_the_received_to_flushed_span() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let slow = settings(
            FakeSourceSettings {
                name: "a".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                write_delay_ms: 20,
                fail_after: Some(60),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, slow, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        let measured =
            snapshot_until(&handle, |s| s.input.is_some_and(|i| i.latency.is_some())).await;
        let input = measured.input.unwrap();
        let Latency {
            capture_ms,
            output_ms,
        } = input.latency.unwrap();
        // At least the write itself; the rest is processing.
        assert!(output_ms >= 20.0, "{output_ms}");
        assert!(output_ms < 200.0, "{output_ms}");
        // Frames without a capture time have no capture part.
        assert_eq!(capture_ms, None);
        // One write at a time, about 50 a second.
        assert!(
            input.sent_fps > 0.0 && input.sent_fps <= 55.0,
            "{}",
            input.sent_fps
        );

        // The 61st write fails: only flushed frames are timed.
        snapshot_until(&handle, |s| s.session.failure().is_some()).await;
        let stats = handle.stats();
        assert_eq!(stats.flushed.count, stats.frames_sent);
        assert_eq!(stats.frames_sent, 60);
        assert!(stats.frames_received > stats.frames_sent);
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// A device that stops showing the colours is reported, not fought: the
/// session keeps writing, and the report goes with the session.
#[tokio::test]
async fn output_issues_are_reported_while_lighting_keeps_running() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let overridden = settings(
            FakeSourceSettings {
                name: "s".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "o".into(),
                overridden_after: Some(20),
                brightness: Some(40),
                device_led_count: Some(120),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, overridden, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        let reported = snapshot_until(&handle, |s| s.device.issue.is_some()).await;
        assert_eq!(reported.device.issue, Some(OutputIssue::Overridden));
        assert_eq!(
            reported.device.brightness,
            DeviceBrightness::from_percent(40),
            "the device's own brightness goes with the issue"
        );
        assert_eq!(
            reported.device.led_count,
            Some(120),
            "and so does its own LED count"
        );
        assert!(
            matches!(reported.session, SessionState::ScreenSync),
            "{}",
            reported.session
        );
        assert!(reported.session.failure().is_none());
        let writes = log.count("output.write:o");
        assert!(
            log.wait_for("output.write:o", writes + 10).await,
            "still writing"
        );

        handle.set_target(LightingTarget::Off).await.unwrap();
        assert_eq!(handle.snapshot().borrow().device, DeviceReport::default());
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// Wait until `input.behind` is `wanted`, giving up after `seconds`.
async fn behind_within(handle: &EngineHandle, wanted: Behind, seconds: u64) -> bool {
    tokio::time::timeout(
        Duration::from_secs(seconds),
        handle
            .snapshot()
            .wait_for(|s| s.input.is_some_and(|input| input.behind == wanted)),
    )
    .await
    .is_ok()
}

/// Lights that take longer to write than the source takes to send the next.
/// The one end-to-end check of `behind`: which step is named, and when a
/// slow source is not behind, is tested on the windows in `meter.rs`.
#[tokio::test]
async fn a_slow_output_is_named_after_a_few_seconds() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let slow = settings(
            FakeSourceSettings {
                name: "a".into(),
                // No wait between frames: a short sleep overruns on a loaded
                // machine and would count as slack.
                immediate: true,
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                // Short against the one-second window, so a write cut off at
                // a window's edge leaves it saturated.
                write_delay_ms: 40,
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, slow, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(behind_within(&handle, Behind::Output, 8).await);
        engine.shutdown().await.unwrap();
    })
    .await;
}
