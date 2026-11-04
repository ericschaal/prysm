use super::*;
use prysm_webos_protocol::PixelFormat;

type Frames = fn(&mut Vec<u8>) -> Result<Option<Header>>;
fn frame(output: &mut Vec<u8>) -> Result<Option<Header>> {
    *output = vec![16, 16, 16, 16, 128, 128];
    Ok(Some(crate::pixels::header(2, 2, PixelFormat::Nv12)?))
}
fn config(source: Source) -> Config {
    let mut config = Config::new("http://127.0.0.1:8787".parse().unwrap());
    config.source = source;
    config.capture.width = 2;
    config.capture.height = 2;
    config
}

#[test]
fn both_taps_wait_for_video_then_preserve_native_pixels_and_provenance() {
    for source in [Source::Both, Source::Video] {
        let mut events = Events::disabled();
        let mut capture = Capture::<Frames>::new(&config(source));
        capture.set_video_available(false);
        let attempts = std::cell::Cell::new(0);
        let mut open = |_: &CaptureOptions| {
            attempts.set(attempts.get() + 1);
            Ok(frame as Frames)
        };
        for _ in 0..3 {
            assert!(capture.capture(&mut open, &mut events).unwrap().is_none());
        }
        assert_eq!(attempts.get(), 0);
        capture.video_retry = Instant::now() + Duration::from_secs(60);
        capture.set_video_available(true);
        let first = capture.capture(&mut open, &mut events).unwrap().unwrap();
        assert_eq!(
            first.content,
            if source == Source::Both {
                Content::Blended
            } else {
                Content::Video
            }
        );
        assert_eq!(first.header.format, PixelFormat::Nv12);
        assert_eq!(first.pixels, [16, 16, 16, 16, 128, 128]);
        assert_eq!(attempts.get(), 1);
        capture.set_video_available(false);
        assert!(capture.capture(&mut open, &mut events).unwrap().is_none());
        assert_eq!(attempts.get(), 1);
    }
}

#[test]
fn failed_startup_backs_off_and_settings_changes_retry_with_new_geometry() {
    let mut events = Events::disabled();
    let mut capture = Capture::<Frames>::new(&config(Source::Both));
    let mut attempts = Vec::new();
    let mut open = |options: &CaptureOptions| {
        attempts.push(*options);
        anyhow::bail!("not ready")
    };
    assert!(capture.capture(&mut open, &mut events).unwrap().is_none());
    capture.video_retry = Instant::now() + Duration::from_secs(60);
    capture.configure(30, [2, 2], &mut events).unwrap();
    assert!(capture.capture(&mut open, &mut events).unwrap().is_none());
    capture.configure(15, [4, 2], &mut events).unwrap();
    assert!(capture.capture(&mut open, &mut events).unwrap().is_none());
    assert_eq!(
        attempts,
        [
            CaptureOptions {
                width: 2,
                height: 2,
                fps: 30
            },
            CaptureOptions {
                width: 4,
                height: 2,
                fps: 15
            }
        ]
    );
}

#[test]
fn constructor_failure_exits_instead_of_retrying_another_backend() {
    let mut capture = Capture::<Frames>::new(&config(Source::Both));
    let error = capture
        .capture(
            &mut |_| {
                Err(crate::native::FatalVideo(anyhow::anyhow!("constructor exception")).into())
            },
            &mut Events::disabled(),
        )
        .err()
        .unwrap();
    assert!(error.is::<crate::native::FatalVideo>());
}

#[test]
fn pending_and_failed_capture_do_not_republish_the_previous_frame() {
    let calls = std::cell::Cell::new(0);
    let mut capture = Capture::new(&config(Source::Both));
    let mut open = |_: &CaptureOptions| {
        Ok(|output: &mut Vec<u8>| {
            calls.set(calls.get() + 1);
            match calls.get() {
                1 => frame(output),
                2 => Ok(None),
                _ => anyhow::bail!("capture failed"),
            }
        })
    };
    let mut events = Events::disabled();
    assert!(capture.capture(&mut open, &mut events).unwrap().is_some());
    assert!(capture.capture(&mut open, &mut events).unwrap().is_none());
    assert!(capture.capture(&mut open, &mut events).unwrap().is_none());
    assert!(capture.video.is_none());
    assert!(capture.video_retry > Instant::now());
}
