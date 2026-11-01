use super::*;
use crate::native::events::Events;
use clap::Parser;
use prysm_webos_protocol::PixelFormat;

fn config(source: &str) -> Cli {
    let mut cli = Cli::try_parse_from(["capture", "127.0.0.1:19400", "--source", source]).unwrap();
    cli.capture.width = 2;
    cli.capture.height = 2;
    cli
}

#[test]
fn ui_frames_keep_their_metadata_and_reuse_storage_without_publishing_stale_pixels() {
    let mut events = Events::disabled();
    let mut capture = Capture::new(&config("ui"));
    let mut open_video = |_: &CaptureOptions| panic!("UI-only capture opened video");
    let mut count = 0;
    let mut ui = Some(|width, height, bgra: &mut Bgra8Pixels| {
        assert_eq!((width, height), (2, 2));
        count += 1;
        if count == 3 {
            anyhow::bail!("UI unavailable");
        }
        *bgra.bytes_mut() = [0, 0, count * 40, 255].repeat(4);
        Ok(())
    });
    let first = capture
        .capture(&mut open_video, &mut ui, &mut events)
        .unwrap()
        .unwrap();
    assert_eq!(first.content, Content::Ui);
    assert_eq!(first.header.format, PixelFormat::Rgb);
    assert_eq!((first.header.width, first.header.height), (2, 2));
    assert_eq!(first.pixels, [40, 0, 0].repeat(4));
    let storage = first.pixels.as_ptr();
    let second = capture
        .capture(&mut open_video, &mut ui, &mut events)
        .unwrap()
        .unwrap();
    assert_eq!(second.pixels, [80, 0, 0].repeat(4));
    assert_eq!(second.pixels.as_ptr(), storage);
    assert!(
        capture
            .capture(&mut open_video, &mut ui, &mut events)
            .unwrap()
            .is_none()
    );
}

#[test]
fn video_startup_backoff_keeps_ui_available_until_a_retry_is_due() {
    let mut events = Events::disabled();
    let mut capture = Capture::new(&config("both"));
    let mut attempts = 0;
    let mut open_video = |_: &CaptureOptions| {
        attempts += 1;
        anyhow::bail!("video unavailable")
    };
    let mut ui = Some(|_, _, bgra: &mut Bgra8Pixels| {
        *bgra.bytes_mut() = [0, 0, 255, 255].repeat(4);
        Ok(())
    });
    assert_eq!(
        capture
            .capture(&mut open_video, &mut ui, &mut events)
            .unwrap()
            .unwrap()
            .content,
        Content::Ui
    );
    capture.video_retry = Instant::now() + Duration::from_secs(60);
    assert_eq!(
        capture
            .capture(&mut open_video, &mut ui, &mut events)
            .unwrap()
            .unwrap()
            .content,
        Content::Ui
    );
    capture.video_retry = Instant::now();
    assert_eq!(
        capture
            .capture(&mut open_video, &mut ui, &mut events)
            .unwrap()
            .unwrap()
            .content,
        Content::Ui
    );
    assert_eq!(attempts, 2);
}

#[test]
fn a_new_rate_retries_video_immediately_with_the_negotiated_options() {
    let mut events = Events::disabled();
    let mut capture = Capture::new(&config("video"));
    let mut ui = None::<fn(u32, u32, &mut Bgra8Pixels) -> Result<()>>;
    capture.video_retry = Instant::now() + Duration::from_secs(60);
    capture.configure(30, [2, 2], &mut ui, &mut events).unwrap();
    assert!(
        capture
            .capture(
                &mut |_| panic!("same rate reset backoff"),
                &mut ui,
                &mut events
            )
            .unwrap()
            .is_none()
    );
    capture.configure(15, [2, 2], &mut ui, &mut events).unwrap();
    let mut rates = Vec::new();
    let mut open_video = |options: &CaptureOptions| {
        rates.push(options.fps);
        anyhow::bail!("video unavailable")
    };
    assert!(
        capture
            .capture(&mut open_video, &mut ui, &mut events)
            .unwrap()
            .is_none()
    );
    assert_eq!(rates, [15]);
}

#[test]
fn resolution_changes_reconfigure_ui_and_retry_video_without_changing_rate() {
    struct RecordingUi {
        configured: Vec<[u32; 2]>,
    }
    impl UiCapture for RecordingUi {
        fn configure(&mut self, options: &CaptureOptions) -> Result<()> {
            self.configured.push([options.width, options.height]);
            Ok(())
        }
        fn capture(&mut self, width: u32, height: u32, output: &mut Bgra8Pixels) -> Result<bool> {
            assert_eq!(self.configured.last(), Some(&[width, height]));
            *output.bytes_mut() = [0, 0, 255, 255].repeat((width * height) as usize);
            Ok(true)
        }
    }
    let mut events = Events::disabled();
    let mut capture = Capture::new(&config("both"));
    let mut ui = Some(RecordingUi {
        configured: Vec::new(),
    });
    let mut attempts = Vec::new();
    let mut open_video = |options: &CaptureOptions| {
        attempts.push([options.width, options.height]);
        anyhow::bail!("test video unavailable")
    };
    capture.configure(30, [2, 2], &mut ui, &mut events).unwrap();
    let first = capture
        .capture(&mut open_video, &mut ui, &mut events)
        .unwrap()
        .unwrap();
    assert_eq!((first.header.width, first.header.height), (2, 2));
    capture.video_retry = Instant::now() + Duration::from_secs(60);
    capture.configure(30, [4, 2], &mut ui, &mut events).unwrap();
    let next = capture
        .capture(&mut open_video, &mut ui, &mut events)
        .unwrap()
        .unwrap();
    assert_eq!((next.header.width, next.header.height), (4, 2));
    assert_eq!(next.pixels.len(), 4 * 2 * 3);
    capture.configure(15, [4, 2], &mut ui, &mut events).unwrap();
    assert_eq!(ui.unwrap().configured, [[2, 2], [4, 2]]);
    assert_eq!(attempts, [[2, 2], [4, 2]]);
}

#[test]
fn constructor_failure_exits_instead_of_retrying_or_sending_ui() {
    let mut events = Events::disabled();
    let mut capture = Capture::new(&config("both"));
    let mut ui =
        Some(|_, _, _: &mut Bgra8Pixels| panic!("UI continued after poisoned video construction"));
    let error = capture
        .capture(
            &mut |_| {
                Err(crate::native::FatalVideo(anyhow::anyhow!("constructor exception")).into())
            },
            &mut ui,
            &mut events,
        )
        .err()
        .unwrap();
    assert!(error.is::<crate::native::FatalVideo>());
}

#[test]
fn absent_video_defers_initialization_keeps_ui_and_retries_immediately_on_return() {
    let mut events = Events::disabled();
    for source in ["both", "video"] {
        let mut capture = Capture::new(&config(source));
        capture.set_video_available(false);
        let mut ui = (source == "both").then_some(|_, _, bgra: &mut Bgra8Pixels| {
            *bgra.bytes_mut() = [0, 0, 255, 255].repeat(4);
            Ok(())
        });
        let mut attempts = 0;
        let mut open = |_: &CaptureOptions| {
            attempts += 1;
            anyhow::bail!("test unavailable")
        };
        for _ in 0..3 {
            let frame = capture.capture(&mut open, &mut ui, &mut events).unwrap();
            assert_eq!(frame.is_some(), source == "both");
        }
        capture.video_retry = Instant::now() + Duration::from_secs(60);
        capture.set_video_available(true);
        capture.capture(&mut open, &mut ui, &mut events).unwrap();
        assert_eq!(attempts, 1);
    }
}
