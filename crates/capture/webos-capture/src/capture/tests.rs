use super::*;
use clap::Parser;
use prysm_webos_protocol::PixelFormat;

fn config(source: &str) -> Cli {
    Cli::try_parse_from([
        "capture",
        "127.0.0.1:19400",
        "--source",
        source,
        "--width",
        "2",
        "--height",
        "2",
    ])
    .unwrap()
}

#[test]
fn ui_frames_keep_their_metadata_and_reuse_storage_without_publishing_stale_pixels() {
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
    let first = capture.capture(&mut open_video, &mut ui).unwrap().unwrap();
    assert_eq!(first.content, Content::Ui);
    assert_eq!(first.header.format, PixelFormat::Rgb);
    assert_eq!((first.header.width, first.header.height), (2, 2));
    assert_eq!(first.pixels, [40, 0, 0].repeat(4));
    let storage = first.pixels.as_ptr();
    let second = capture.capture(&mut open_video, &mut ui).unwrap().unwrap();
    assert_eq!(second.pixels, [80, 0, 0].repeat(4));
    assert_eq!(second.pixels.as_ptr(), storage);
    assert!(capture.capture(&mut open_video, &mut ui).unwrap().is_none());
}

#[test]
fn video_startup_backoff_keeps_ui_available_until_a_retry_is_due() {
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
            .capture(&mut open_video, &mut ui)
            .unwrap()
            .unwrap()
            .content,
        Content::Ui
    );
    capture.video_retry = Instant::now() + Duration::from_secs(60);
    assert_eq!(
        capture
            .capture(&mut open_video, &mut ui)
            .unwrap()
            .unwrap()
            .content,
        Content::Ui
    );
    capture.video_retry = Instant::now();
    assert_eq!(
        capture
            .capture(&mut open_video, &mut ui)
            .unwrap()
            .unwrap()
            .content,
        Content::Ui
    );
    assert_eq!(attempts, 2);
}

#[test]
fn a_new_rate_retries_video_immediately_with_the_negotiated_options() {
    let mut capture = Capture::new(&config("video"));
    let mut ui = None::<fn(u32, u32, &mut Bgra8Pixels) -> Result<()>>;
    capture.video_retry = Instant::now() + Duration::from_secs(60);
    capture.configure_rate(30);
    assert!(
        capture
            .capture(&mut |_| panic!("same rate reset backoff"), &mut ui)
            .unwrap()
            .is_none()
    );
    capture.configure_rate(15);
    let mut rates = Vec::new();
    let mut open_video = |options: &CaptureOptions| {
        rates.push(options.fps);
        anyhow::bail!("video unavailable")
    };
    assert!(capture.capture(&mut open_video, &mut ui).unwrap().is_none());
    assert_eq!(rates, [15]);
}
