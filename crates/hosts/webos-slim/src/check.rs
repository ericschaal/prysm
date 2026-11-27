//! `prysm-webos-slim check-encoder`: a bounded on-TV check of the configured
//! H.264 encoder with synthetic pictures, and optionally a stream of them to
//! a Prysm. Not used by the supervisor.
use clap::{Parser, ValueEnum};
use prysm_capture_remote::RemoteSender;
use prysm_core::Frame;
use prysm_h264::{Backend, Decoder, Encoder, EncoderSettings, FORMAT};
use prysm_ports::{FrameSink, chain};
use prysm_webos_link::PrysmAddress;
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// Side of the flat squares of [`picture`]: one macroblock.
const BLOCK: usize = 16;
/// Pictures until [`picture`] repeats, more than the five each check tells
/// apart.
const PERIOD: u32 = 8;
/// Largest accepted mean absolute difference of a decoded block from its own
/// colour: far above OpenH264's 0.3 on these flat blocks, far below the 28
/// levels between neighbouring pictures, and catches a shifted range.
const TOLERANCE: f64 = 8.0;

const AFTER_HELP: &str = "\
Encodes synthetic NV12 pictures with exactly that encoder, decodes each one and
checks that every 16x16 block shows its own picture, not one up to two before
or after, restarts the session half way (as on a keyframe request), and
prints timings and sizes. With --prysm, then streams the same pictures to that
Prysm for --frames more frames, at the default bitrate; its remote capture must
request this encoding, size and rate. A sending TV worker is replaced: it fails,
and the TV asks to try again. Exit 0 when every check passed.";

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
    #[arg(
        long,
        default_value_t = prysm_h264::DEFAULT_BITRATE,
        value_parser = clap::value_parser!(u32).range(
            i64::from(*prysm_h264::BITRATE.start())..=i64::from(*prysm_h264::BITRATE.end())
        )
    )]
    bitrate: u32,
    /// Then stream the pictures to this Prysm (http:// or https://)
    #[arg(long, value_name = "URL")]
    prysm: Option<PrysmAddress>,
}

/// The encoders `check-encoder` can check: the H.264 encodings of `StreamEncoding`.
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
            eprintln!("check-encoder: FAILED: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Limited-range BT.709 NV12, as the TV capture delivers, of flat 16x16
/// blocks whose colour steps through 8 levels, 28 apart, per picture (and per
/// block, left to right), so a block decoded from a neighbouring picture, or a picture torn
/// between two, cannot pass as this one (a moving gradient would: neighbours
/// differ too little on average).
pub fn picture(width: u32, height: u32, index: u32) -> Frame {
    let (w, h) = (width as usize, height as usize);
    let blocks = w.div_ceil(BLOCK);
    let block = |row: usize, col: usize| row / BLOCK * blocks + col / BLOCK;
    let mut data: Vec<u8> = (0..w * h)
        .map(|i| colour(index, block(i / w, i % w))[0])
        .collect();
    for row in (0..h).step_by(2) {
        for col in (0..w).step_by(2) {
            let [_, u, v] = colour(index, block(row, col));
            data.extend([u, v]);
        }
    }
    Frame::new(data, width, height, FORMAT).expect("valid NV12 geometry")
}

/// The flat Y, U, V of `block` in picture `index`: one of [`PERIOD`] levels
/// each, offset so the three planes do not move in step.
fn colour(index: u32, block: usize) -> [u8; 3] {
    let level = |step: usize| 16 + 28 * (step % PERIOD as usize) as u8;
    let step = index as usize + block;
    [level(step), level(step + 3), level(step + 5)]
}

/// Checks every block of a decoded `picture(width, height, index)`: it must
/// be within [`TOLERANCE`] of its colour and nearer to it than to the same
/// block two pictures before to two after. Returns the worst block's error.
fn compare(decoded: &[u8], width: u32, height: u32, index: u32) -> Result<f64, String> {
    let (w, h) = (width as usize, height as usize);
    let blocks = w.div_ceil(BLOCK);
    let mut worst = 0.0_f64;
    for block in 0..blocks * h.div_ceil(BLOCK) {
        // Picture `index + PERIOD` looks like `index`, so the pictures
        // before the first need no negative index.
        let at = (block % blocks * BLOCK, block / blocks * BLOCK);
        let error = |offset: i32| {
            let other = (index + PERIOD).wrapping_add_signed(offset);
            block_error(decoded, w, h, at, colour(other, block))
        };
        let own = error(0);
        if let Some(offset) = [-2, -1, 1, 2]
            .into_iter()
            .find(|&offset| error(offset) <= own)
        {
            return Err(format!(
                "the block at {at:?} shows picture {offset:+} instead"
            ));
        }
        if own > TOLERANCE {
            return Err(format!(
                "the block at {at:?} decodes {own:.1} away from its colour"
            ));
        }
        worst = worst.max(own);
    }
    Ok(worst)
}

/// The mean absolute difference of the block at `(left, top)` (luma and
/// chroma) of an NV12 picture from a flat colour.
fn block_error(
    pixels: &[u8],
    w: usize,
    h: usize,
    (left, top): (usize, usize),
    [y, u, v]: [u8; 3],
) -> f64 {
    let (rows, cols) = (top..h.min(top + BLOCK), left..w.min(left + BLOCK));
    let (luma, chroma) = pixels.split_at(w * h);
    let (mut sum, mut count) = (0_u64, 0_u64);
    for row in rows.clone() {
        for col in cols.clone() {
            sum += u64::from(luma[row * w + col].abs_diff(y));
            count += 1;
        }
    }
    // Blocks start on even rows and columns: one U, V pair per 2x2 pixels.
    for row in rows.step_by(2) {
        for col in cols.clone().step_by(2) {
            let pair = row / 2 * w + col;
            sum += u64::from(chroma[pair].abs_diff(u)) + u64::from(chroma[pair + 1].abs_diff(v));
            count += 2;
        }
    }
    sum as f64 / count as f64
}

/// User plus system CPU ticks of this process (Linux), at 100 per second.
fn cpu_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

fn run(options: Options) -> Result<(), String> {
    let Options {
        encoding,
        width,
        height,
        fps,
        frames,
        bitrate,
        prysm,
    } = options;
    let backend = encoding.backend();
    let settings = EncoderSettings {
        backend,
        fps,
        bitrate,
    };
    eprintln!(
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
        let error = compare(&decoded, width, height, index)
            .map_err(|error| format!("frame {index}: {error} (keyframe: {})", picture.keyframe))?;
        worst = worst.max(error);
        if index < 3 || picture.keyframe {
            eprintln!(
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
    eprintln!(
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
    eprintln!(
        "check-encoder: released in {} ms",
        started.elapsed().as_millis()
    );
    if let Some(prysm) = prysm {
        stream(&prysm, settings, width, height, frames)?;
    }
    Ok(())
}

/// The real sending path: the same encoder inside a `RemoteSender`, if
/// Prysm requests it.
fn stream(
    prysm: &PrysmAddress,
    settings: EncoderSettings,
    width: u32,
    height: u32,
    frames: u32,
) -> Result<(), String> {
    let address = match prysm.socket_addr() {
        Some(address) => address,
        None => prysm_ports::system_lookup(prysm.host(), prysm.port())
            .map_err(|error| format!("cannot resolve {prysm}: {error}"))?,
    };
    let mut sender = RemoteSender::connect(
        &prysm.capture_stream(),
        address,
        &crate::capture::offer(&[prysm_capture_webos::MODE]),
    )
    .map_err(|error| chain(&error))?;
    let request = sender.request();
    let checked = (Some(settings.backend), width, height, settings.fps);
    if (
        Backend::of(request.encoding),
        request.width,
        request.height,
        request.fps,
    ) != checked
    {
        sender.refuse("check-encoder streams only what its flags say");
        return Err(format!(
            "Prysm requests {request}; give check-encoder the same \
             --encoding, --width, --height and --fps"
        ));
    }
    let interval = Duration::from_secs_f64(1.0 / f64::from(settings.fps));
    for index in 0..frames {
        let tick = Instant::now();
        sender
            .send(&picture(width, height, index))
            .map_err(|error| format!("frame {index}: {}", chain(&error)))?;
        std::thread::sleep(interval.saturating_sub(tick.elapsed()));
    }
    eprintln!("check-encoder: streamed {frames} frames to {prysm}");
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
            &["--prysm", "nope"],
            &["--prysm", "http://0.0.0.0:8787"],
            &["--fps"],
            &["--colour", "red"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn only_the_own_picture_passes_the_comparison() {
        let (width, height) = (40, 24);
        let pixels = |index| picture(width, height, index).as_slice().to_vec();
        for index in [0, 1, 5, 9] {
            assert_eq!(compare(&pixels(index), width, height, index), Ok(0.0));
        }
        for wrong in [8, 9, 11, 12] {
            let error = compare(&pixels(wrong), width, height, 10).unwrap_err();
            assert!(error.contains("shows picture"), "{wrong}: {error}");
        }
        let (mut torn, next) = (pixels(10), pixels(11));
        let w = width as usize;
        let luma = w * height as usize;
        torn[w * 16..luma].copy_from_slice(&next[w * 16..luma]);
        torn[luma + w * 8..].copy_from_slice(&next[luma + w * 8..]);
        let error = compare(&torn, width, height, 10).unwrap_err();
        assert!(error.contains("(0, 16)"), "{error}");
        let shifted: Vec<u8> = pixels(10).iter().map(|&value| value + 9).collect();
        let error = compare(&shifted, width, height, 10).unwrap_err();
        assert!(error.contains("away from its colour"), "{error}");
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
