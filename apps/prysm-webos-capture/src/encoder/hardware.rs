use anyhow::{Result, ensure};
use prysm_metrics::Stage;
use prysm_webos_protocol::{
    Header, PixelFormat,
    h264::{FrameKind, hardware::normalize_access_unit},
};

#[allow(unsafe_code)]
#[path = "gst.rs"]
mod gst;

pub(crate) struct Hardware {
    pipeline: gst::Pipeline,
    width: u32,
    height: u32,
    encoded: Vec<u8>,
    first: bool,
}
impl Hardware {
    pub(super) fn new(header: Header, fps: u8, bitrate: u32) -> Result<Self> {
        ensure!(
            header.format == PixelFormat::Nv12,
            "hardware capture requires NV12"
        );
        header.raw_len()?;
        ensure!(
            header.width.is_multiple_of(2) && header.height.is_multiple_of(2),
            "H.264 needs even dimensions"
        );
        let stride = (header.width as usize).div_ceil(192) * 192;
        Ok(Self {
            pipeline: gst::Pipeline::new(header.width, header.height, stride, fps, bitrate)?,
            width: header.width,
            height: header.height,
            encoded: Vec::new(),
            first: true,
        })
    }
    pub(super) fn encode(
        &mut self,
        header: Header,
        pixels: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<FrameKind> {
        ensure!(
            header.width == self.width && header.height == self.height,
            "hardware input dimensions changed"
        );
        ensure!(
            header.format == PixelFormat::Nv12 && pixels.len() == header.raw_len()?,
            "invalid hardware NV12 input"
        );
        self.pipeline
            .encode(pixels, header.width as usize, &mut self.encoded)?;
        let _time = prysm_metrics::timer(Stage::HardwareNormalize);
        let kind =
            normalize_access_unit(&self.encoded, self.width, self.height, self.first, output)?;
        self.first = false;
        Ok(kind)
    }
}

#[cfg(any(target_os = "linux", test))]
fn copy_nv12(
    width: usize,
    height: usize,
    stride: usize,
    input: &[u8],
    output: &mut [u8],
) -> Result<()> {
    ensure!(
        (2..=1920).contains(&width)
            && (2..=1080).contains(&height)
            && width.is_multiple_of(2)
            && height.is_multiple_of(2)
            && stride >= width
            && stride <= 1920
            && input.len() == width * height * 3 / 2
            && output.len() == stride * height * 3 / 2,
        "invalid NV12 copy layout"
    );
    for (source, destination) in input
        .chunks_exact(width)
        .zip(output.chunks_exact_mut(stride))
    {
        destination[..width].copy_from_slice(source);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_nv12_upload_rewrites_active_rows_and_preserves_padding() {
        for stride in [64, 192] {
            let mut destination = vec![0xa5; stride * 34 * 3 / 2];
            for value in [16, 235] {
                let input = vec![value; 64 * 34 * 3 / 2];
                copy_nv12(64, 34, stride, &input, &mut destination).unwrap();
                for row in destination.chunks_exact(stride) {
                    assert_eq!(&row[..64], &[value; 64]);
                    assert!(row[64..].iter().all(|v| *v == 0xa5));
                }
            }
            let unchanged = destination.clone();
            for (width, height, pitch, length) in [
                (0, 34, stride, 0),
                (63, 34, stride, 63 * 34 * 3 / 2),
                (64, 33, stride, 64 * 33 * 3 / 2),
                (64, 34, 63, 64 * 34 * 3 / 2),
                (64, 34, stride, 64 * 34 * 3 / 2 - 1),
            ] {
                assert!(
                    copy_nv12(width, height, pitch, &vec![0; length], &mut destination).is_err()
                );
                assert_eq!(destination, unchanged);
            }
        }
    }
}

#[cfg(test)]
mod qualification {
    use super::*;
    use std::time::{Duration, Instant};
    fn ticks() -> u64 {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
    }
    fn capture_input(header: Header, rgb: &[u8]) -> Vec<u8> {
        use yuv::{
            BufferStoreMut, YuvBiPlanarImageMut, YuvConversionMode, YuvRange, YuvStandardMatrix,
        };
        assert_eq!(header.format, PixelFormat::Nv12);
        let n = header.width as usize * header.height as usize;
        let mut pixels = vec![0; n * 3 / 2];
        let (y, uv) = pixels.split_at_mut(n);
        let mut image = YuvBiPlanarImageMut {
            y_plane: BufferStoreMut::Borrowed(y),
            y_stride: header.width,
            uv_plane: BufferStoreMut::Borrowed(uv),
            uv_stride: header.width,
            width: header.width,
            height: header.height,
        };
        yuv::rgb_to_yuv_nv12(
            &mut image,
            rgb,
            header.width * 3,
            YuvRange::Limited,
            YuvStandardMatrix::Bt709,
            YuvConversionMode::Balanced,
        )
        .unwrap();
        pixels
    }
    fn metadata_and_failure_reset() {
        use crate::encoder::{Encoder, HardwareFailed};
        use prysm_metrics::Content;
        use prysm_webos_protocol::Compression;
        let header = Header {
            width: 320,
            height: 180,
            format: PixelFormat::Nv12,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 0,
        };
        let red = [255, 0, 0].repeat(320 * 180);
        let input = capture_input(header, &red);
        let mut encoder = Encoder::new(Compression::H264Hardware, 15, 2_000_000).unwrap();
        let mut decoder = prysm_webos_protocol::h264::Decoder::new().unwrap();
        let packet = encoder
            .encode(header, &input, Content::Video)
            .unwrap()
            .expect("first hardware input must produce its own picture");
        assert_eq!(packet.content, Content::Video);
        assert_eq!((packet.header.width, packet.header.height), (320, 180));
        let rgb = decoder.decode(packet.header, packet.pixels).unwrap();
        assert!(
            rgb[0] > 240 && rgb[2] < 15,
            "first wire picture must be the current red input"
        );
        assert!(encoder.hardware_active());
        let blue = capture_input(header, &[0, 0, 255].repeat(320 * 180));
        let packet = encoder
            .encode(header, &blue, Content::Blended)
            .unwrap()
            .unwrap();
        assert_eq!(
            packet.content,
            Content::Blended,
            "metadata must belong to the current input"
        );
        let rgb = decoder.decode(packet.header, packet.pixels).unwrap();
        assert!(
            rgb[2] > 240 && rgb[0] < 15,
            "second wire picture must be the current blue input"
        );
        let changed = Header {
            width: 640,
            height: 360,
            ..header
        };
        let green = capture_input(changed, &[0, 255, 0].repeat(640 * 360));
        let error = encoder
            .encode(changed, &green, Content::Video)
            .err()
            .expect("geometry change must reset connection");
        assert!(error.downcast_ref::<HardwareFailed>().is_some());
        drop(encoder);
        let mut software = Encoder::new(Compression::H264Software, 15, 2_000_000).unwrap();
        let packet = software
            .encode(changed, &green, Content::Video)
            .unwrap()
            .unwrap();
        let rgb = prysm_webos_protocol::h264::Decoder::new()
            .unwrap()
            .decode(packet.header, packet.pixels)
            .unwrap();
        assert!(
            rgb[1] > 240 && rgb[0] < 15,
            "software reconnect must begin independently"
        );
    }
    #[test]
    #[ignore = "TV only; requires idle capture lock and external process watchdog"]
    #[allow(clippy::too_many_lines)]
    fn hardware_capture_qualification() {
        let _lock = crate::capture_lock(std::path::Path::new("/tmp/prysm-capture.lock")).unwrap();
        // Recreate both codec and decoder at every geometry change/reconnect.
        for (width, height, format, fps) in [
            (640, 360, PixelFormat::Nv12, 15),
            (320, 180, PixelFormat::Nv12, 15),
            (640, 360, PixelFormat::Nv12, 30),
        ] {
            let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
            let header = Header {
                width,
                height,
                format,
                codec: prysm_webos_protocol::Codec::Raw,
                payload_len: (width * height * 3) as usize,
            };
            let mut pixels: Vec<_> = (0..height)
                .flat_map(|_| {
                    (0..width).flat_map(|x| match x * 4 / width {
                        0 => [255, 0, 0],
                        1 => [0, 255, 0],
                        2 => [0, 0, 255],
                        _ => [64, 64, 64],
                    })
                })
                .collect();
            let mut hardware = Hardware::new(header, fps, 2_000_000).unwrap();
            let mut decoder = prysm_webos_protocol::h264::Decoder::new().unwrap();
            let mut output = Vec::new();
            let cpu = ticks();
            let start = Instant::now();
            for index in 0..30 {
                for row in pixels.chunks_exact_mut(width as usize * 3) {
                    row[width as usize * 9 / 4..].fill(if index < 15 {
                        64 + index * 5
                    } else {
                        32 + (index % 2) * 160
                    });
                }
                let input = capture_input(header, &pixels);
                let now = Instant::now();
                let kind = hardware.encode(header, &input, &mut output).unwrap();
                if index == 0 {
                    assert_eq!(kind, FrameKind::Idr);
                }
                eprintln!("capture_to_au_us={}", now.elapsed().as_micros());
                eprintln!(
                    "hardware {width}x{height} frame={index} encode_us={}",
                    now.elapsed().as_micros()
                );
                let rgb = decoder
                    .decode(
                        Header {
                            format: PixelFormat::Rgb,
                            codec: prysm_webos_protocol::Codec::H264,
                            payload_len: output.len(),
                            ..header
                        },
                        &output,
                    )
                    .unwrap();
                for x in [width / 8, width * 3 / 8, width * 5 / 8, width * 7 / 8] {
                    let offset = ((height / 2 * width + x) * 3) as usize;
                    for c in 0..3 {
                        let expected = pixels[offset + c];
                        let mut sum = 0u32;
                        for y in height / 2 - 12..height / 2 + 12 {
                            for col in x - 12..x + 12 {
                                sum += u32::from(rgb[((y * width + col) * 3) as usize + c]);
                            }
                        }
                        let mean = f64::from(sum) / 576.0;
                        assert!(
                            (mean - f64::from(expected)).abs() <= 8.0,
                            "hardware patch format={format:?} frame={index} x={x} c={c} mean={mean} expected={expected}"
                        );
                    }
                }
                std::thread::sleep(interval.saturating_sub(now.elapsed()));
            }
            eprintln!(
                "hardware {format:?} {width}x{height} cpu_ticks={} wall_ms={} (includes receiver decode)",
                ticks() - cpu,
                start.elapsed().as_millis()
            );
            let now = Instant::now();
            drop(hardware);
            eprintln!("hardware teardown_us={}", now.elapsed().as_micros());
            let mut software = prysm_webos_protocol::h264::Encoder::new(2_000_000, fps).unwrap();
            let mut decoder = prysm_webos_protocol::h264::Decoder::new().unwrap();
            let cpu = ticks();
            let start = Instant::now();
            for index in 0..30 {
                for row in pixels.chunks_exact_mut(width as usize * 3) {
                    row[width as usize * 9 / 4..].fill(if index < 15 {
                        64 + index * 5
                    } else {
                        32 + (index % 2) * 160
                    });
                }
                let input = capture_input(header, &pixels);
                let now = Instant::now();
                software.encode(header, &input, &mut output).unwrap();
                eprintln!(
                    "software {width}x{height} encode_us={}",
                    now.elapsed().as_micros()
                );
                decoder
                    .decode(
                        Header {
                            format: PixelFormat::Rgb,
                            codec: prysm_webos_protocol::Codec::H264,
                            payload_len: output.len(),
                            ..header
                        },
                        &output,
                    )
                    .unwrap();
                std::thread::sleep(interval.saturating_sub(now.elapsed()));
            }
            eprintln!(
                "software {format:?} {width}x{height} cpu_ticks={} wall_ms={}",
                ticks() - cpu,
                start.elapsed().as_millis()
            );
        }
        metadata_and_failure_reset();
        if std::env::var_os("PRYSM_QUALIFY_REAL_CAPTURE").is_some() {
            let mut video = crate::native::Video::open(
                &crate::config::CaptureOptions {
                    width: 640,
                    height: 360,
                    fps: 15,
                },
                crate::config::Source::Both,
            )
            .unwrap();
            let mut events = crate::events::Events::disabled();
            let mut pixels = Vec::new();
            let mut encoder = None;
            let mut decoder = prysm_webos_protocol::h264::Decoder::new().unwrap();
            let mut output = Vec::new();
            let start = Instant::now();
            let cpu = ticks();
            let mut count = 0;
            while count < 30 && start.elapsed() < Duration::from_secs(6) {
                if let Some(header) = video.capture(&mut pixels, &mut events).unwrap() {
                    if encoder.is_none() {
                        encoder = Some(Hardware::new(header, 15, 2_000_000).unwrap());
                    }
                    let now = Instant::now();
                    encoder
                        .as_mut()
                        .unwrap()
                        .encode(header, &pixels, &mut output)
                        .unwrap();
                    eprintln!(
                        "real capture {:?} encode_us={}",
                        header.format,
                        now.elapsed().as_micros()
                    );
                    decoder
                        .decode(
                            Header {
                                format: PixelFormat::Rgb,
                                codec: prysm_webos_protocol::Codec::H264,
                                payload_len: output.len(),
                                ..header
                            },
                            &output,
                        )
                        .unwrap();
                    count += 1;
                }
                std::thread::sleep(Duration::from_millis(67));
            }
            assert_eq!(count, 30, "real capture did not deliver frames");
            eprintln!(
                "real capture cpu_ticks={} wall_ms={}",
                ticks() - cpu,
                start.elapsed().as_millis()
            );
            drop(encoder);
            drop(video);
            eprintln!("real capture teardown completed");
        }
    }
}
