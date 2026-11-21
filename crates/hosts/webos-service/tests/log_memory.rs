//! An over-long stderr line is skipped, not kept: memory stays bounded. A
//! binary of its own, because it reads the whole process's peak memory.
use nix::sys::resource::{UsageWho, getrusage};
use prysm_webos_service::launcher::{Launcher, Output, ProcessLauncher, Worker};
use std::path::Path;
use std::time::{Duration, Instant};

const LINE: usize = 64 * 1024 * 1024;

/// The process's peak resident memory in bytes.
fn peak() -> i64 {
    // `c_long`: 32 bits on the TV.
    #[allow(clippy::useless_conversion)]
    let rss = i64::from(getrusage(UsageWho::RUSAGE_SELF).unwrap().max_rss());
    // macOS counts bytes, Linux KiB.
    if cfg!(target_os = "macos") {
        rss
    } else {
        rss * 1024
    }
}

#[test]
fn a_multi_megabyte_log_line_without_newline_is_skipped_in_bounded_memory() {
    let script = format!("head -c {LINE} /dev/zero | tr '\\0' y >&2; echo >&2; echo done >&2");
    let before = peak();
    let mut worker = ProcessLauncher
        .spawn(Path::new("/bin/sh"), &["-c".into(), script])
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut seen = Vec::new();
    while !seen.contains(&Output::Log("done".into())) {
        assert!(Instant::now() < deadline, "only saw {} lines", seen.len());
        seen.extend(worker.drain());
        std::thread::sleep(Duration::from_millis(10));
    }
    let grown = peak() - before;
    assert!(
        grown < 16 * 1024 * 1024,
        "{grown} bytes for a {LINE}-byte line"
    );
    assert!(matches!(&seen[0], Output::Log(line) if line.len() == 512));
}
