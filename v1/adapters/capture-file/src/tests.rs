use super::*;
use crate::source::{Deadlines, FileFrames, STDERR_TAIL, ffmpeg};
use prysm_core::{Frame, PixelFormat};
use prysm_ports::{AdapterSettings, Pull, Registry, Settings};
use std::error::Error as _;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
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

/// Run `work` on its own thread; fail the test instead of hanging.
fn within<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, result) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(work());
    });
    result
        .recv_timeout(LIMIT)
        .expect("playback made no progress")
}

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

/// The next frame, waiting past any `Idle`; `None` once playback ended.
fn pull(frames: &mut FileFrames) -> prysm_ports::Result<Option<Frame>> {
    let deadline = Instant::now() + LIMIT;
    loop {
        match frames.next_frame(SHORT)? {
            Pull::Frame(frame) => return Ok(Some(frame)),
            Pull::Ended => return Ok(None),
            Pull::Idle => assert!(Instant::now() < deadline, "no frame"),
        }
    }
}

/// The next frame, waiting past any `Idle`.
fn next(frames: &mut FileFrames) -> prysm_ports::Result<Frame> {
    Ok(pull(frames)?.expect("playback ended"))
}

fn chain(error: &Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// The adapter error's preserved cause, downcast.
fn cause<E: std::error::Error + 'static>(error: &Error) -> Option<&E> {
    match error {
        Error::Adapter {
            source: Some(source),
            ..
        } => source.downcast_ref(),
        _ => None,
    }
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn frames_have_the_requested_size_and_format_then_playback_finishes() {
    within(|| {
        let script = format!("{}; {}", frame_of('A'), frame_of('B'));
        let mut frames = spawn(&script);
        for fill in *b"AB" {
            let frame = next(&mut frames).unwrap();
            assert_eq!((frame.width(), frame.height()), (WIDTH, HEIGHT));
            assert_eq!(frame.format(), PixelFormat::RGB24);
            assert_eq!(frame.as_slice(), &[fill; 96]);
        }
        // The end of the file is completion, not a failure.
        assert!(pull(&mut frames).unwrap().is_none());
        // Terminal: repeated, not retried.
        assert!(matches!(frames.next_frame(SHORT), Ok(Pull::Ended)));
        Box::new(frames).close().unwrap();
    });
}

#[test]
fn released_buffers_are_reused() {
    within(|| {
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
    within(|| {
        let script = format!("{}; sleep 0.3; {}", frame_of('A'), frame_of('B'));
        let mut frames = spawn(&script);
        next(&mut frames).unwrap();
        let first_at = Instant::now();
        // Nothing buffered or invented between paced frames.
        assert!(matches!(frames.next_frame(SHORT), Ok(Pull::Idle)));
        next(&mut frames).unwrap();
        assert!(first_at.elapsed() >= Duration::from_millis(200));
        Box::new(frames).close().unwrap();
    });
}

#[test]
fn timeout_returns_idle() {
    within(|| {
        let mut frames = spawn("exec sleep 30");
        let started = Instant::now();
        assert!(matches!(frames.next_frame(SHORT), Ok(Pull::Idle)));
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
    within(|| {
        let script = "echo 'first line' >&2; i=0; while [ $i -lt 400 ]; do echo \"noise $i\" >&2; \
                      i=$((i+1)); done; echo 'clip.mkv: Invalid data found' >&2; exit 1";
        let mut frames = spawn(script);
        let error = next(&mut frames).unwrap_err();
        assert_eq!(error.to_string(), "cannot play video file `clip.mkv`");
        // A failure is terminal too.
        assert_eq!(
            frames.next_frame(SHORT).unwrap_err().to_string(),
            error.to_string()
        );
        let exit = cause::<crate::source::DecoderExit>(&error).unwrap();
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
    within(|| {
        let mut frames = spawn("head -c 50 /dev/zero; echo 'truncated' >&2");
        let error = next(&mut frames).unwrap_err();
        assert_eq!(
            chain(&error),
            "cannot play video file `clip.mkv`: output ended mid-frame (50 of 96 bytes); \
             FFmpeg exited with exit status: 0; stderr: truncated"
        );
        Box::new(frames).close().unwrap();
    });
}

#[test]
fn stall_mid_frame_is_a_failure_and_close_still_reaps() {
    within(|| {
        let mut frames = spawn("head -c 50 /dev/zero; exec sleep 30");
        let pid = frames.pid();
        let error = next(&mut frames).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("FFmpeg stalled mid-frame playing `clip.mkv`: 50 of 96 bytes"),
            "{error}"
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
    let io = cause::<std::io::Error>(&error).unwrap();
    assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn close_kills_and_reaps_within_the_bound() {
    within(|| {
        let frames = spawn("exec sleep 30");
        let pid = frames.pid();
        assert!(alive(pid));
        let started = Instant::now();
        Box::new(frames).close().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        // Reaped: no zombie left behind.
        assert!(!alive(pid));
    });
}

#[test]
fn drop_kills_and_reaps() {
    within(|| {
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
    assert_eq!(field(|s| s.path = " ".into()), "capture.path");
    assert_eq!(field(|s| s.path = "a\0b".into()), "capture.path");
    assert_eq!(field(|s| s.width = 0), "capture.width");
    assert_eq!(field(|s| s.width = 3841), "capture.width");
    assert_eq!(field(|s| s.height = 0), "capture.height");
    assert_eq!(field(|s| s.height = 2161), "capture.height");
    assert_eq!(field(|s| s.fps = 0), "capture.fps");
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
    let io = cause::<std::io::Error>(&error).unwrap();
    assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
}

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Runs against the real FFmpeg when it is on PATH; skips otherwise.
#[test]
fn real_ffmpeg_plays_once_paced_and_reports_bad_files() {
    if !ffmpeg_available() {
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
    let played = within(move || {
        let mut source = FileSource.open(&settings).unwrap();
        let mut count = 0;
        let mut first_at = None;
        let deadline = Instant::now() + LIMIT;
        loop {
            assert!(Instant::now() < deadline);
            match source.next_frame(SHORT).unwrap() {
                Pull::Frame(frame) => {
                    assert_eq!((frame.width(), frame.height()), (32, 18));
                    assert_eq!(frame.len(), 32 * 18 * 3);
                    first_at.get_or_insert_with(Instant::now);
                    count += 1;
                }
                Pull::Idle => {}
                Pull::Ended => break,
            }
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

    // With `repeat` the three frames play again and the source does not end.
    let mut settings = FileSettings::new(clip.to_str().unwrap());
    settings.width = 32;
    settings.height = 18;
    settings.repeat = true;
    let repeated = within(move || {
        let mut source = FileSource.open(&settings).unwrap();
        let mut count = 0;
        let deadline = Instant::now() + LIMIT;
        while count < 7 {
            assert!(Instant::now() < deadline);
            match source.next_frame(SHORT).unwrap() {
                Pull::Frame(_) => count += 1,
                Pull::Idle => {}
                Pull::Ended => break,
            }
        }
        source.close().unwrap();
        count
    });
    assert_eq!(repeated, 7, "a repeating file keeps playing past its end");

    let invalid = dir.path().join("invalid.mkv");
    std::fs::write(&invalid, b"invalid video data").unwrap();
    let settings = FileSettings::new(invalid.to_str().unwrap());
    let error = within(move || {
        let mut source = FileSource.open(&settings).unwrap();
        let deadline = Instant::now() + LIMIT;
        loop {
            assert!(Instant::now() < deadline);
            if let Err(error) = source.next_frame(SHORT) {
                source.close().unwrap();
                return chain(&error);
            }
        }
    });
    assert!(error.contains("FFmpeg exited with"), "{error}");
    assert!(error.contains("stderr: "), "{error}");
}
