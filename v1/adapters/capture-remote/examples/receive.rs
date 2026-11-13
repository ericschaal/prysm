//! A desktop receiver for checking a TV's stream without lighting anything:
//!
//! ```sh
//! cargo run --manifest-path v1/Cargo.toml -p prysm-capture-remote --example receive -- \
//!   0.0.0.0:7380 [SECONDS] [--pause-at N] [--width N] [--height N] [--fps N] \
//!   [--encoding h264_hardware|h264_software|raw]
//! ```
//!
//! Asks the sender for that stream (default 320x180 at 30 fps, H.264 from
//! the hardware encoder, menus included). Prints one line per second: frames
//! decoded, size, format and the mean luma of the newest frame, or the
//! sender's refusal. `--pause-at N` pauses the sender at second N and resumes
//! it three seconds later. Exits after SECONDS (default 30).
use prysm_capture_remote::{RemoteSource, StreamRequest};
use prysm_ports::{FrameSource, chain};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let listen = args
        .first()
        .ok_or("usage: receive ADDR [SECONDS] [--pause-at N] [--width N] [--height N] [--fps N] [--encoding E]")?;
    let seconds: u64 = args
        .get(1)
        .filter(|arg| !arg.starts_with("--"))
        .map_or(Ok(30), |s| s.parse())?;
    let option = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
    };
    let pause_at: Option<u64> = option("--pause-at").map(|n| n.parse()).transpose()?;
    let mut request = StreamRequest::default();
    if let Some(width) = option("--width") {
        request.width = width.parse()?;
    }
    if let Some(height) = option("--height") {
        request.height = height.parse()?;
    }
    if let Some(fps) = option("--fps") {
        request.fps = fps.parse()?;
    }
    if let Some(encoding) = option("--encoding") {
        request.encoding = encoding.parse()?;
    }
    request.validate()?;
    let mut source = RemoteSource::bind(listen.parse()?, request)?;
    println!(
        "receive: listening on {} for {request}",
        source.local_addr()
    );
    let started = Instant::now();
    for second in 0..seconds {
        if pause_at == Some(second) {
            source.set_wanted(false);
            println!("receive: pausing the sender");
        }
        if pause_at.map(|at| at + 3) == Some(second) {
            source.set_wanted(true);
            println!("receive: resuming the sender");
        }
        let until = started + Duration::from_secs(second + 1);
        let (mut frames, mut last) = (0, None);
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            let pull = match source.next_frame(left) {
                Ok(pull) => pull,
                Err(error) => {
                    println!("receive: {}", chain(&error));
                    continue;
                }
            };
            if let Some(frame) = pull.into_frame() {
                frames += 1;
                last = Some(frame);
            }
        }
        match last {
            Some(frame) => {
                let luma = frame.width() as usize * frame.height() as usize;
                let mean = frame.as_slice()[..luma.min(frame.len())]
                    .iter()
                    .map(|&v| u64::from(v))
                    .sum::<u64>()
                    / luma.max(1) as u64;
                println!(
                    "receive: second={second} frames={frames} {}x{} {} mean_first_plane={mean}",
                    frame.width(),
                    frame.height(),
                    frame.format()
                );
            }
            None => println!("receive: second={second} frames=0"),
        }
    }
    Box::new(source).close()?;
    Ok(())
}
