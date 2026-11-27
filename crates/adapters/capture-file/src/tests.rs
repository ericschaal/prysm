use super::*;
use crate::players;
use crate::source::{Deadlines, DecoderExit, Ended, FileFrames, STDERR_TAIL, ffmpeg};
use nix::sys::signal::kill;
use nix::unistd::{Pid, getpgid};
use prysm_core::{Frame, PixelFormat};
use prysm_ports::{AdapterSettings, Pull, Registry, Settings, chain};
use prysm_testkit::within;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(10);
const SHORT: Duration = Duration::from_millis(50);
const TEST: Deadlines = Deadlines {
    stall: Duration::from_millis(300),
    exit: Duration::from_secs(5),
    reap: Duration::from_secs(5),
};
/// An 8×4 RGB24 frame is 96 bytes.
const WIDTH: u32 = 8;
const HEIGHT: u32 = 4;

/// A fake decoder: a shell script writing raw frames to stdout.
fn fake(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    command
}

/// Shell that writes one 8×4 frame filled with the ASCII byte `fill`.
fn frame_of(fill: char) -> String {
    format!("head -c 96 /dev/zero | tr '\\000' '{fill}'")
}

fn spawn(script: &str) -> FileFrames {
    FileFrames::spawn(fake(script), WIDTH, HEIGHT, "clip.mkv", TEST).unwrap()
}

/// The next frame, waiting past any quiet pull; `None` once playback ended.
fn pull(frames: &mut dyn FrameSource) -> prysm_ports::Result<Option<Frame>> {
    let deadline = Instant::now() + LIMIT;
    loop {
        match frames.next_frame(SHORT)? {
            Pull::Frame(frame) => return Ok(Some(frame)),
            Pull::Ended => return Ok(None),
            Pull::Quiet => assert!(Instant::now() < deadline, "no frame"),
            Pull::Refused => panic!("a file is never refused"),
        }
    }
}

/// The next frame, waiting past any quiet pull.
fn next(frames: &mut dyn FrameSource) -> prysm_ports::Result<Frame> {
    Ok(pull(frames)?.expect("playback ended"))
}

fn alive(pid: u32) -> bool {
    kill(Pid::from_raw(pid.try_into().unwrap()), None).is_ok()
}

#[test]
fn playback_keeps_the_requested_frame_size_and_format_after_stderr_closes() {
    within(LIMIT, || {
        let script = format!("exec 2>&-; {}; {}", frame_of('A'), frame_of('B'));
        let mut frames = spawn(&script);
        for fill in *b"AB" {
            let frame = next(&mut frames).unwrap();
            assert_eq!((frame.width(), frame.height()), (WIDTH, HEIGHT));
            assert_eq!(frame.format(), PixelFormat::RGB24);
            assert_eq!(frame.as_slice(), &[fill; 96]);
        }
        assert!(pull(&mut frames).unwrap().is_none());
        assert!(matches!(frames.next_frame(SHORT), Ok(Pull::Ended)));
        Box::new(frames).close().unwrap();
    });
}

#[test]
fn released_buffers_are_reused() {
    within(LIMIT, || {
        let script = format!("{}; {}", frame_of('A'), frame_of('B'));
        let mut frames = spawn(&script);
        let first = next(&mut frames).unwrap();
        let pixels = first.as_slice().as_ptr();
        drop(first);
        let second = next(&mut frames).unwrap();
        assert_eq!(second.as_slice().as_ptr(), pixels);
        Box::new(frames).close().unwrap();
    });
}

#[test]
fn frames_are_delivered_as_the_decoder_paces_them() {
    within(LIMIT, || {
        let script = format!(
            "{}; sleep 0.3; head -c 48 /dev/zero; sleep 0.05; \
                             head -c 48 /dev/zero | tr '\\000' B",
            frame_of('A')
        );
        let mut frames = spawn(&script);
        next(&mut frames).unwrap();
        let first_at = Instant::now();
        assert!(matches!(frames.next_frame(SHORT), Ok(Pull::Quiet)));
        let second = next(&mut frames).unwrap();
        assert_eq!(&second.as_slice()[..48], &[0; 48]);
        assert_eq!(&second.as_slice()[48..], &[b'B'; 48]);
        assert!(first_at.elapsed() >= Duration::from_millis(200));
        Box::new(frames).close().unwrap();
    });
}

#[test]
fn timeout_returns_quiet() {
    within(LIMIT, || {
        let mut frames = spawn("exec sleep 30");
        let started = Instant::now();
        assert!(matches!(frames.next_frame(SHORT), Ok(Pull::Quiet)));
        let waited = started.elapsed();
        assert!(
            waited >= SHORT && waited < Duration::from_secs(1),
            "{waited:?}"
        );
        Box::new(frames).close().unwrap();
    });
}

#[test]
fn failure_reports_exit_status_and_a_bounded_stderr_tail() {
    within(LIMIT, || {
        let script = "echo 'first line' >&2; i=0; while [ $i -lt 400 ]; do echo \"noise $i\" >&2; \
                      i=$((i+1)); done; echo 'clip.mkv: Invalid data found' >&2; exit 1";
        let mut frames = spawn(script);
        let error = next(&mut frames).unwrap_err();
        assert_eq!(error.to_string(), "cannot play video file `clip.mkv`");
        assert_eq!(
            frames.next_frame(SHORT).unwrap_err().to_string(),
            error.to_string()
        );
        let exit = error.cause::<DecoderExit>().unwrap();
        assert!(
            matches!(&exit.ended, Ended::Exited(status) if status.code() == Some(1)),
            "{exit:?}"
        );
        assert_eq!(exit.partial, None);
        assert!(exit.stderr.starts_with('…'));
        let cause = exit.to_string();
        assert!(
            cause.starts_with("FFmpeg exited with exit status: 1; stderr: …"),
            "{cause}"
        );
        assert!(
            cause.ends_with("noise 399 | clip.mkv: Invalid data found"),
            "{cause}"
        );
        assert!(!cause.contains("first line"));
        assert!(cause.len() < 2 * STDERR_TAIL, "{}", cause.len());
        Box::new(frames).close().unwrap();
    });
}

#[test]
fn output_ending_mid_frame_is_a_failure() {
    within(LIMIT, || {
        let mut frames = spawn("head -c 50 /dev/zero; echo 'truncated' >&2");
        let error = next(&mut frames).unwrap_err();
        assert_eq!(
            chain(&error),
            "cannot play video file `clip.mkv`: output ended mid-frame (50 of 96 bytes); \
             FFmpeg exited with exit status: 0; stderr: truncated"
        );
        let exit = error.cause::<DecoderExit>().unwrap();
        assert!(matches!(&exit.ended, Ended::Exited(status) if status.success()));
        assert_eq!(exit.partial, Some((50, 96)));
        Box::new(frames).close().unwrap();
    });
}

#[test]
fn stall_mid_frame_is_a_failure_and_close_still_reaps() {
    within(LIMIT, || {
        let mut frames = spawn("head -c 50 /dev/zero; exec sleep 30");
        let pid = frames.pid();
        let error = next(&mut frames).unwrap_err();
        assert_eq!(error.to_string(), "cannot play video file `clip.mkv`");
        assert_eq!(
            error.cause::<DecoderExit>().unwrap().ended,
            Ended::Stalled {
                filled: 50,
                expected: 96,
                within: TEST.stall
            }
        );
        let again = frames.next_frame(SHORT).unwrap_err();
        assert!(again.cause::<DecoderExit>().is_some());
        Box::new(frames).close().unwrap();
        assert!(!alive(pid));
    });
}

/// A process left behind by the decoder may hold stderr open after the
/// decoder exited: playback still ends at the exit deadline, as finished.
#[test]
fn a_leftover_holding_stderr_does_not_hide_the_exit() {
    within(LIMIT, || {
        let deadlines = Deadlines {
            exit: Duration::from_millis(300),
            ..TEST
        };
        let script = format!("{}; exec >&-; sleep 3 & exit 0", frame_of('A'));
        let mut frames =
            FileFrames::spawn(fake(&script), WIDTH, HEIGHT, "clip.mkv", deadlines).unwrap();
        next(&mut frames).unwrap();
        assert!(pull(&mut frames).unwrap().is_none());
        Box::new(frames).close().unwrap();
    });
}

#[test]
fn a_decoder_that_closes_its_output_but_does_not_exit_fails_at_the_deadline() {
    within(LIMIT, || {
        let deadlines = Deadlines {
            exit: Duration::from_millis(300),
            ..TEST
        };
        let mut frames = FileFrames::spawn(
            fake("exec >&- 2>&-; exec sleep 30"),
            WIDTH,
            HEIGHT,
            "clip.mkv",
            deadlines,
        )
        .unwrap();
        let pid = frames.pid();
        let error = pull(&mut frames).unwrap_err();
        let exit = error.cause::<DecoderExit>().unwrap();
        assert_eq!(exit.ended, Ended::DidNotExit(deadlines.exit));
        // Still listed, so the host can kill it; `close` does.
        assert!(players::listed(pid));
        Box::new(frames).close().unwrap();
        assert!(!alive(pid));
    });
}

#[test]
fn the_decoder_runs_in_a_process_group_of_its_own() {
    within(LIMIT, || {
        let frames = spawn("exec sleep 30");
        let pid = frames.pid();
        let child = Pid::from_raw(pid.try_into().unwrap());
        assert_eq!(getpgid(Some(child)).unwrap(), child);
        assert_ne!(pid, std::process::id());
        assert!(players::listed(pid));
        Box::new(frames).close().unwrap();
        assert!(!players::listed(pid));
    });
}

const KILL_CHILD: &str = "PRYSM_CAPTURE_FILE_KILL_CHILD";

/// Runs [`kill_remaining_child`] alone in a child process: `kill_remaining`
/// would also kill the decoders of tests running beside it.
#[test]
fn kill_remaining_kills_decoders_still_running() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::kill_remaining_child", "--ignored"])
        .env(KILL_CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains("1 passed"),
        "the child test did not run: {stdout}"
    );
}

#[test]
#[ignore = "run by kill_remaining_kills_decoders_still_running in a child process"]
fn kill_remaining_child() {
    if std::env::var_os(KILL_CHILD).is_none() {
        return;
    }
    within(LIMIT, || {
        let mut frames = spawn("exec sleep 30");
        let pid = frames.pid();
        crate::kill_remaining();
        assert!(!players::listed(pid));
        let error = pull(&mut frames).unwrap_err();
        let ended = &error.cause::<DecoderExit>().unwrap().ended;
        assert!(
            matches!(ended, Ended::Exited(status) if status.signal() == Some(9)),
            "{ended:?}"
        );
        Box::new(frames).close().unwrap();
        assert!(!alive(pid));
    });
}

#[test]
fn missing_binary_names_the_requirement() {
    let Err(error) = FileFrames::spawn(
        Command::new("prysm-test-missing-ffmpeg"),
        WIDTH,
        HEIGHT,
        "clip.mkv",
        TEST,
    ) else {
        panic!("a missing decoder cannot start");
    };
    assert!(
        error
            .to_string()
            .contains("install FFmpeg 9+ and make it available on PATH"),
        "{error}"
    );
    let io = error.cause::<std::io::Error>().unwrap();
    assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn close_kills_and_reaps_within_the_bound() {
    within(LIMIT, || {
        let frames = spawn("exec sleep 30");
        let pid = frames.pid();
        assert!(alive(pid));
        let started = Instant::now();
        Box::new(frames).close().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!alive(pid));
    });
}

#[test]
fn drop_kills_and_reaps() {
    within(LIMIT, || {
        let frames = spawn("exec sleep 30");
        let pid = frames.pid();
        drop(frames);
        assert!(!alive(pid));
    });
}

#[test]
fn ffmpeg_command_matches_the_shipping_pipeline() {
    let mut settings = FileSettings::new("/videos/clip.mkv");
    settings.width = 320;
    settings.height = 180;
    settings.fps = 25;
    let command = ffmpeg(&settings);
    assert_eq!(command.get_program(), "ffmpeg");
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_str().unwrap())
        .collect();
    assert_eq!(
        args,
        [
            "-nostdin",
            "-v",
            "error",
            "-xerror",
            "-i",
            "file:/videos/clip.mkv",
            "-map",
            "0:v:0",
            "-vf",
            "fps='min(source_fps,25)',scale=320:180:flags=area:intent=perceptual:\
             out_primaries=bt709:out_transfer=srgb:out_range=full,realtime",
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "rgb24",
            "-threads:v",
            "1",
            "-f",
            "rawvideo",
            "pipe:1",
        ]
    );
    settings.repeat = true;
    let args: Vec<_> = ffmpeg(&settings)
        .get_args()
        .map(|arg| arg.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        args[..8],
        [
            "-nostdin",
            "-v",
            "error",
            "-xerror",
            "-stream_loop",
            "-1",
            "-i",
            "file:/videos/clip.mkv"
        ]
    );
}

#[test]
fn settings_file_shape_and_defaults() {
    let settings: Settings =
        serde_json::from_str(r#"{ "capture": { "kind": "file", "path": "/videos/clip.mkv" } }"#)
            .unwrap();
    let mut registry = Registry::default();
    registry.add_source(FileSource);
    registry.validate(&settings).unwrap();
    let decoded: FileSettings =
        serde_json::from_value(serde_json::to_value(settings.capture.unwrap().fields).unwrap())
            .unwrap();
    assert_eq!(decoded, FileSettings::new("/videos/clip.mkv"));
    assert_eq!((decoded.width, decoded.height, decoded.fps), (640, 360, 30));
    assert!(!decoded.repeat, "plays once by default");
    let repeating: FileSettings =
        serde_json::from_str(r#"{ "path": "/a", "repeat": true }"#).unwrap();
    assert!(repeating.repeat);
    let encoded = AdapterSettings::new(FileSource::KIND, &decoded);
    assert_eq!(encoded.kind, "file");
    assert!(
        serde_json::from_str::<FileSettings>(r#"{ "path": "/a", "loop": true }"#).is_err(),
        "unknown fields are rejected"
    );
}

#[test]
fn settings_validation_names_the_field() {
    let field = |change: fn(&mut FileSettings)| {
        let mut settings = FileSettings::new("/videos/clip.mkv");
        change(&mut settings);
        FileSource.validate(&settings).unwrap_err().field
    };
    assert_eq!(field(|s| s.path = " ".into()), "path");
    assert_eq!(field(|s| s.path = "a\0b".into()), "path");
    assert_eq!(field(|s| s.width = 0), "width");
    assert_eq!(field(|s| s.width = 3841), "width");
    assert_eq!(field(|s| s.height = 0), "height");
    assert_eq!(field(|s| s.height = 2161), "height");
    assert_eq!(field(|s| s.fps = 0), "fps");
    FileSource
        .validate(&FileSettings::new("/videos/clip.mkv"))
        .unwrap();
}

#[test]
fn open_reports_an_unreadable_file_before_starting_ffmpeg() {
    let Err(error) = FileSource.open(&FileSettings::new("/prysm-test-missing/clip.mkv")) else {
        panic!("a missing file cannot open");
    };
    assert_eq!(
        error.to_string(),
        "cannot open video file `/prysm-test-missing/clip.mkv`"
    );
    let io = error.cause::<std::io::Error>().unwrap();
    assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
}

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Runs against the real FFmpeg when it is on PATH; skips otherwise, except
/// in CI, which installs it.
#[test]
fn real_ffmpeg_plays_once_paced_and_reports_bad_files() {
    if !ffmpeg_available() {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI installs ffmpeg, but it is not on PATH"
        );
        eprintln!("skipped: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("three-frames.mkv");
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi"])
        .args(["-i", "testsrc=size=16x16:rate=10"])
        .args(["-frames:v", "3", "-c:v", "ffv1"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(status.success());
    let mut settings = FileSettings::new(clip.to_str().unwrap());
    settings.width = 32;
    settings.height = 18;
    let played = within(LIMIT, move || {
        let mut source = FileSource.open(&settings).unwrap();
        let mut count = 0;
        let mut first_at = None;
        while let Some(frame) = pull(&mut *source).unwrap() {
            assert_eq!((frame.width(), frame.height()), (32, 18));
            assert_eq!(frame.len(), 32 * 18 * 3);
            first_at.get_or_insert_with(Instant::now);
            count += 1;
        }
        source.close().unwrap();
        (count, first_at.unwrap().elapsed())
    });
    let (count, span) = played;
    assert_eq!(count, 3);
    assert!(
        span >= Duration::from_millis(150),
        "frames came in a burst: {span:?}"
    );

    let mut settings = FileSettings::new(clip.to_str().unwrap());
    settings.width = 32;
    settings.height = 18;
    settings.repeat = true;
    within(LIMIT, move || {
        let mut source = FileSource.open(&settings).unwrap();
        // The clip has three frames. Seven frames require two rewinds.
        for _ in 0..7 {
            next(&mut *source).unwrap();
        }
        source.close().unwrap();
    });

    let invalid = dir.path().join("invalid.mkv");
    std::fs::write(&invalid, b"invalid video data").unwrap();
    let settings = FileSettings::new(invalid.to_str().unwrap());
    let error = within(LIMIT, move || {
        let mut source = FileSource.open(&settings).unwrap();
        loop {
            if let Err(error) = source.next_frame(SHORT) {
                source.close().unwrap();
                return chain(&error);
            }
        }
    });
    assert!(error.contains("FFmpeg exited with"), "{error}");
    assert!(error.contains("stderr: "), "{error}");
}
