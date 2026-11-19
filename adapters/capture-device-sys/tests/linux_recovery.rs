#![cfg(target_os = "linux")]

//! Use a dedicated v4l2loopback device with no other readers or writers:
//! `sudo modprobe v4l2loopback video_nr=42 exclusive_caps=1`
//! `PRYSM_TEST_VIDEO_DEVICE=/dev/video42 cargo test -p prysm-capture-device-sys --test linux_recovery -- --ignored`

use anyhow::{Context, Result, bail, ensure};
use prysm_capture_device_sys::{CaptureSink, Frame, capture};
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};
use v4l::control::{Control, Value};
use v4l::video::Output;
use v4l::{Device, Format, FourCC};

/// The newest frame and whether to stop, as a host's source keeps them.
#[derive(Default)]
struct Latest {
    state: Mutex<(Option<Frame>, bool)>,
    changed: Condvar,
}

impl Latest {
    fn lock(&self) -> MutexGuard<'_, (Option<Frame>, bool)> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The newest frame, waiting up to `wait` for one.
    fn take(&self, wait: Duration) -> Option<Frame> {
        let (mut state, _) = self
            .changed
            .wait_timeout_while(self.lock(), wait, |(frame, _)| frame.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        state.0.take()
    }

    fn stop(&self) {
        self.lock().1 = true;
        self.changed.notify_all();
    }
}

impl CaptureSink for Latest {
    fn ready(&self) {}

    fn frame(&self, frame: Frame) {
        self.lock().0 = Some(frame);
        self.changed.notify_all();
    }

    fn should_stop(&self, wait: Duration) -> bool {
        self.changed
            .wait_timeout_while(self.lock(), wait, |(_, stop)| !*stop)
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .1
    }
}

#[test]
#[ignore = "requires a dedicated v4l2loopback device in PRYSM_TEST_VIDEO_DEVICE"]
fn stalled_producer_recovers_and_capture_stops() -> Result<()> {
    let path = std::env::var("PRYSM_TEST_VIDEO_DEVICE")
        .context("Set PRYSM_TEST_VIDEO_DEVICE to a dedicated v4l2loopback device")?;
    let mut producer = Device::with_path(&path)?;
    ensure!(
        producer.query_caps()?.driver == "v4l2 loopback",
        "Expected a v4l2loopback device"
    );
    for control in producer.query_controls()? {
        if matches!(control.name.as_str(), "sustain_framerate" | "timeout") {
            producer.set_control(Control {
                id: control.id,
                value: Value::Integer(0),
            })?;
        }
    }
    let format = producer.set_format(&Format::new(64, 48, FourCC::new(b"YUYV")))?;
    ensure!(
        format.width == 64 && format.height == 48 && format.fourcc == FourCC::new(b"YUYV"),
        "Loopback device must accept 64x48 YUYV"
    );
    producer.write_all(&[16, 128, 16, 128].repeat(64 * 48 / 2))?;

    let latest = Arc::new(Latest::default());
    let sink: Arc<dyn CaptureSink> = latest.clone();
    let device = path.clone();
    let capturing =
        thread::spawn(move || capture(Some(&device), 64, 48, 30.try_into().unwrap(), &sink));
    for luma in [16, 235] {
        let pixels = [luma, 128, luma, 128].repeat(64 * 48 / 2);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            ensure!(
                Instant::now() < deadline,
                "Capture did not receive the producer's new frame"
            );
            ensure!(
                !capturing.is_finished(),
                "Capture ended before the producer's new frame arrived"
            );
            if latest
                .take(Duration::from_millis(30))
                .is_some_and(|frame| frame.as_slice() == pixels)
            {
                break;
            }
            producer.write_all(&pixels)?;
        }

        // The producer pauses through two one-second capture timeouts.
        thread::sleep(Duration::from_millis(2200));
        ensure!(
            !capturing.is_finished(),
            "Capture ended while the producer was paused"
        );
    }

    latest.stop();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !capturing.is_finished() {
        if Instant::now() >= deadline {
            bail!("Stopping did not end the stalled capture");
        }
        thread::sleep(Duration::from_millis(10));
    }
    match capturing.join() {
        Ok(result) => result.context("capture failed or did not release the device")?,
        Err(_) => bail!("capture panicked"),
    }
    // Reconfiguration must succeed immediately after capture returns.
    Device::with_path(&path)?.set_format(&format)?;
    Ok(())
}
