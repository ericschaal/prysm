//! `prysm-webos-slim check-encoder`: a bounded on-TV check of the configured
//! H.264 encoder with synthetic pictures, and optionally a stream of them to
//! a desktop receiver. Not used by the supervisor.
use clap::{Parser, ValueEnum};
use prysm_capture_remote::RemoteSender;
use prysm_core::Frame;
use prysm_h264::{Backend, Decoder, Encoder, EncoderSettings, FORMAT};
use prysm_ports::FrameSink;
use prysm_webos_link::ReceiverAddress;
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// Largest accepted mean absolute NV12 difference per picture.
const TOLERANCE: f64 = 8.0;

const AFTER_HELP: &str = "\
Encodes synthetic NV12 pictures with exactly that encoder, decodes each one and
compares it with its input, restarts the session half way (as on resume), and
prints timings and sizes. With --receiver, then streams the same pictures to a
Prysm frame receiver for --frames more frames, at the default bitrate; the
receiver must request this encoding, size and rate. Exit 0 when every check
passed.";

#[derive(Debug, Clone, Parser)]
#[command(after_help = AFTER_HELP)]
pub struct Options {
    /// Encoder to check
    #[arg(long, value_enum, default_value_t = Encoding::H264Hardware)]
    encoding: Encoding,
    /// Picture width, even, 16 to 1920
    #[arg(long, default_value_t = 640, value_parser = even(1920))]
    width: u32,
    /// Picture height, even, 16 to 1080
    #[arg(long, default_value_t = 280, value_parser = even(1080))]
    height: u32,
    /// Pictures per second
    #[arg(long, default_value_t = 30)]
    fps: u32,
    /// Pictures to encode (and to stream), 1 to 10000
    #[arg(long, default_value_t = 90, value_parser = clap::value_parser!(u32).range(1..=10_000))]
    frames: u32,
    /// Encoder bitrate in bit/s
    #[arg(long, default_value_t = prysm_h264::DEFAULT_BITRATE)]
    bitrate: u32,
    /// Then stream the pictures to this Prysm frame receiver
    #[arg(long, value_name = "HOST:PORT")]
    receiver: Option<ReceiverAddress>,
}

/// The encoders `check-encoder` can check: the slim encodings except raw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
enum Encoding {
    H264Hardware,
    H264Software,
}

impl Encoding {
    const fn backend(self) -> Backend {
        match self {
            Self::H264Hardware => Backend::Hardware,
            Self::H264Software => Backend::Software,
        }
    }
}

/// An even number from 16 to `max`.
fn even(max: u32) -> impl Fn(&str) -> Result<u32, String> + Clone {
    move |text| match text.parse::<u32>() {
        Ok(n) if (16..=max).contains(&n) && n.is_multiple_of(2) => Ok(n),
        _ => Err(format!("expected an even number from 16 to {max}")),
    }
}

pub fn main(options: Options) -> ExitCode {
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            println!("check-encoder: FAILED: {error}");
            ExitCode::FAILURE
        }
    }
}

/// A moving luma ramp with chroma quadrants that change colour every 15
/// pictures: limited-range BT.709 NV12, as the TV capture delivers.
pub fn picture(width: u32, height: u32, index: u32) -> Frame {
    let (w, h) = (width as usize, height as usize);
    let shift = index as usize * 7;
    let mut data: Vec<u8> = (0..w * h)
        .map(|i| 16 + ((i % w + shift) * 200 / w % 200) as u8)
        .collect();
    let palette = [(90, 240), (54, 34), (240, 110), (128, 128), (200, 60)];
    let phase = (index / 15) as usize;
    for row in 0..h / 2 {
        for col in 0..w / 2 {
            let quadrant = (row * 2 / (h / 2)) * 2 + col * 2 / (w / 2);
            let (u, v) = palette[(quadrant + phase) % palette.len()];
            data.extend([u, v]);
        }
    }
    Frame::new(data, width, height, FORMAT).expect("valid NV12 geometry")
}

fn mean_error(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    sum as f64 / a.len() as f64
}

/// User plus system CPU ticks of this process (Linux), at 100 per second.
fn cpu_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

fn chain(error: &dyn std::error::Error) -> String {
    prysm_ports::chain(error)
}

fn run(options: Options) -> Result<(), String> {
    let Options {
        encoding,
        width,
        height,
        fps,
        frames,
        bitrate,
        receiver,
    } = options;
    let backend = encoding.backend();
    let settings = EncoderSettings {
        backend,
        fps,
        bitrate,
    };
    println!(
        "check-encoder: encoder={} {width}x{height} fps={fps} bitrate={bitrate} frames={frames}",
        backend.name()
    );
    let mut encoder = Encoder::new(settings).map_err(|error| chain(&error))?;
    let mut decoder = None;
    let mut decoded = vec![0; (width * height * 3 / 2) as usize];
    let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
    let (mut bytes, mut keyframes, mut skipped) = (0_usize, 0_u32, 0_u32);
    let (mut encode_total, mut encode_max, mut worst) = (Duration::ZERO, Duration::ZERO, 0.0_f64);
    let cpu = cpu_ticks();
    let started = Instant::now();
    for index in 0..frames {
        let tick = Instant::now();
        let input = picture(width, height, index);
        if index == frames / 2 && index > 0 {
            encoder.restart();
        }
        let encode_started = Instant::now();
        let picture = encoder
            .encode(&input)
            .map_err(|error| format!("frame {index}: {}", chain(&error)))?;
        let took = encode_started.elapsed();
        encode_total += took;
        encode_max = encode_max.max(took);
        let Some(picture) = picture else {
            skipped += 1;
            continue;
        };
        if index == 0 && !picture.keyframe {
            return Err("the first picture is not a keyframe".into());
        }
        if index == frames / 2 && index > 0 && !picture.keyframe {
            return Err("a restarted session did not start with a keyframe".into());
        }
        bytes += picture.data.len();
        if picture.keyframe {
            keyframes += 1;
            decoder = Some(Decoder::new().map_err(|error| chain(&error))?);
        }
        let decoder = decoder
            .as_mut()
            .ok_or("a picture before the first keyframe")?;
        decoder
            .decode(picture.data, width, height, &mut decoded)
            .map_err(|error| format!("frame {index} does not decode: {}", chain(&error)))?;
        let error = mean_error(&decoded, input.as_slice());
        worst = worst.max(error);
        if error > TOLERANCE {
            return Err(format!(
                "frame {index} decodes {error:.1} away from its input (keyframe: {})",
                picture.keyframe
            ));
        }
        if index < 3 || picture.keyframe {
            println!(
                "check-encoder: frame={index} keyframe={} bytes={} encode_us={} error={error:.2}",
                picture.keyframe,
                picture.data.len(),
                took.as_micros()
            );
        }
        std::thread::sleep(interval.saturating_sub(tick.elapsed()));
    }
    let wall = started.elapsed();
    let encoded = frames - skipped;
    let seconds = wall.as_secs_f64();
    let cpu = cpu.zip(cpu_ticks()).map_or_else(
        || "n/a".to_owned(),
        |(before, after)| format!("{:.1}", (after - before) as f64 / seconds),
    );
    println!(
        "check-encoder: ok encoder={} frames={frames} encoded={encoded} skipped={skipped} \
         keyframes={keyframes} bytes={bytes} mbit_s={:.2} encode_ms_mean={:.2} \
         encode_ms_max={:.2} worst_error={worst:.2} cpu_percent={cpu} wall_ms={}",
        backend.name(),
        bytes as f64 * 8.0 / seconds / 1e6,
        encode_total.as_secs_f64() * 1000.0 / f64::from(frames),
        encode_max.as_secs_f64() * 1000.0,
        wall.as_millis()
    );
    let started = Instant::now();
    drop(encoder);
    println!(
        "check-encoder: released in {} ms",
        started.elapsed().as_millis()
    );
    if let Some(receiver) = receiver {
        stream(receiver, settings, width, height, frames)?;
    }
    Ok(())
}

/// The real sending path: the same encoder inside a `RemoteSender`, if the
/// receiver requests it.
fn stream(
    receiver: ReceiverAddress,
    settings: EncoderSettings,
    width: u32,
    height: u32,
    frames: u32,
) -> Result<(), String> {
    let address = match receiver.socket_addr() {
        Some(address) => address,
        None => prysm_ports::system_lookup(receiver.host(), receiver.port())
            .map_err(|error| format!("cannot resolve {receiver}: {error}"))?,
    };
    let mut sender = RemoteSender::connect(address).map_err(|error| chain(&error))?;
    let request = sender.request();
    let checked = (Some(settings.backend), width, height, settings.fps);
    if (
        request.encoding.backend(),
        request.width,
        request.height,
        request.fps,
    ) != checked
    {
        sender.refuse("check-encoder streams only what its flags say");
        return Err(format!(
            "the receiver requests {request}; give check-encoder the same \
             --encoding, --width, --height and --fps"
        ));
    }
    let interval = Duration::from_secs_f64(1.0 / f64::from(settings.fps));
    let mut paused = 0;
    for index in 0..frames {
        let tick = Instant::now();
        sender
            .send(&picture(width, height, index))
            .map_err(|error| format!("frame {index}: {}", chain(&error)))?;
        paused += u32::from(sender.paused());
        std::thread::sleep(interval.saturating_sub(tick.elapsed()));
    }
    println!(
        "check-encoder: streamed {frames} frames to {receiver} ({paused} dropped while paused)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, clap::Error> {
        Options::try_parse_from(["check-encoder"].iter().chain(args))
    }

    #[test]
    fn options_parse_and_validate() {
        let options = parse(&["--encoding", "h264_software", "--frames", "5"]).unwrap();
        assert_eq!(options.encoding, Encoding::H264Software);
        assert_eq!(options.frames, 5);
        let defaults = parse(&[]).unwrap();
        assert_eq!(defaults.encoding, Encoding::H264Hardware);
        assert_eq!((defaults.width, defaults.height), (640, 280));
        for bad in [
            &["--encoding", "raw"][..],
            &["--width", "641"],
            &["--height", "1082"],
            &["--frames", "0"],
            &["--receiver", "nope"],
            &["--receiver", "0.0.0.0:7380"],
            &["--fps"],
            &["--colour", "red"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_software_check_passes_on_the_host() {
        let options = Options {
            frames: 8,
            fps: 60,
            ..parse(&["--encoding", "h264_software"]).unwrap()
        };
        run(options).unwrap();
    }

    #[test]
    fn the_hardware_check_fails_off_the_tv() {
        let error = run(Options {
            frames: 2,
            ..parse(&[]).unwrap()
        })
        .unwrap_err();
        assert!(error.contains("hardware"), "{error}");
    }
}
