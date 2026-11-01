//! Temporary hardware diagnostic, not a service or a public capture API.
//! Run only on an idle TV under an external timeout; see cleanup-diagnosis.md.
#![deny(unsafe_code)]
#![allow(
    dead_code,
    reason = "reuse production modules without exposing a library API"
)]

#[path = "../src/cli.rs"]
mod cli;
#[cfg(not(test))]
#[allow(unsafe_code)]
#[path = "../src/native/mod.rs"]
mod native;
#[cfg(not(test))]
#[path = "../src/pixels.rs"]
mod pixels;
#[path = "../src/shutdown.rs"]
mod shutdown;
use anyhow::Result;
#[cfg(not(test))]
use anyhow::{Context, ensure};
use clap::Parser;
use serde_json::json;
use std::{collections::BTreeMap, fs};

#[derive(Parser)]
struct Probe {
    #[arg(long, value_enum)]
    source: cli::Source,
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..=50))]
    cycles: u32,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(0..=30))]
    frames: u32,
    #[command(flatten)]
    capture: cli::CaptureOptions,
}

fn snapshot(cycle: u32, stage: &str) -> Result<()> {
    let mut fds = BTreeMap::new();
    for entry in fs::read_dir("/proc/self/fd")? {
        let entry = entry?;
        if let Ok(target) = fs::read_link(entry.path()) {
            // Exclude the directory iterator's own temporary descriptor.
            if target != std::path::Path::new(&format!("/proc/{}/fd", std::process::id())) {
                fds.insert(entry.file_name().to_string_lossy().into_owned(), target);
            }
        }
    }
    let mut threads = BTreeMap::new();
    for entry in fs::read_dir("/proc/self/task")? {
        let entry = entry?;
        threads.insert(
            entry.file_name().to_string_lossy().into_owned(),
            json!({"comm": fs::read_to_string(entry.path().join("comm")).ok(),
                   "wchan": fs::read_to_string(entry.path().join("wchan")).ok()}),
        );
    }
    println!(
        "{}",
        json!({"cycle": cycle, "stage": stage, "fds": fds,
        "threads": threads, "status": fs::read_to_string("/proc/self/status")?,
        "maps": fs::read_to_string("/proc/self/maps")?})
    );
    Ok(())
}

#[cfg(not(test))]
fn main() -> Result<()> {
    use std::{os::unix::fs::OpenOptionsExt, time::Duration};
    let probe = Probe::parse();
    ensure!(cfg!(target_os = "linux"), "probe requires webOS Linux");
    ensure!(
        nix::unistd::Uid::effective().is_root(),
        "probe requires root"
    );
    ensure!(!probe.capture.force_capture, "probe must not force capture");
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    // Share the installed worker's lock without changing its service or settings.
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open("/tmp/prysm-capture.lock")?;
    let _lock = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
        .map_err(|(_, error)| error)
        .context("another Prysm capturer is running")?;
    let (stop, _signal_waiter) = shutdown::listen()?;
    let mut events = native::events::Events::from_environment()?;
    snapshot(0, "baseline")?;
    let mut failures = 0;
    for cycle in 1..=probe.cycles {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        snapshot(cycle, "before-open")?;
        let mut ui = None;
        let mut video = None;
        let result = (|| -> Result<()> {
            if probe.source.includes_ui() {
                ui = Some(native::Ui::open(&probe.capture)?);
                snapshot(cycle, "ui-open")?;
            }
            if probe.source.includes_video() {
                video = Some(native::Video::open(&probe.capture)?);
                snapshot(cycle, "video-open")?;
            }
            let mut bgra = pixels::Bgra8Pixels::default();
            let mut bytes = Vec::new();
            for _ in 0..probe.frames {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                if let Some(video) = &mut video {
                    let frame = video.capture(&mut bytes, &mut events)?;
                    println!(
                        "{}",
                        json!({"cycle": cycle, "video_frame": frame.is_some()})
                    );
                }
                if let Some(ui) = &mut ui {
                    ui.capture(probe.capture.width, probe.capture.height, &mut bgra)?;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            snapshot(cycle, "captured")
        })();
        snapshot(cycle, "before-drop")?;
        drop(video);
        snapshot(cycle, "video-dropped")?;
        drop(ui);
        std::thread::sleep(Duration::from_millis(100));
        snapshot(cycle, "ui-dropped")?;
        if let Err(error) = result {
            failures += 1;
            println!("{}", json!({"cycle": cycle, "error": format!("{error:#}")}));
        }
    }
    ensure!(failures == 0, "{failures} capture cycles failed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_requires_explicit_source_and_bounds_work() {
        assert!(Probe::try_parse_from(["probe"]).is_err());
        for invalid in ["0", "51"] {
            assert!(
                Probe::try_parse_from(["probe", "--source", "ui", "--cycles", invalid]).is_err()
            );
        }
        assert!(Probe::try_parse_from(["probe", "--source", "both", "--frames", "31"]).is_err());
        let probe = Probe::try_parse_from(["probe", "--source", "ui", "--frames", "0"]).unwrap();
        assert_eq!(probe.frames, 0);
        assert!(!probe.capture.force_capture);
    }
}
