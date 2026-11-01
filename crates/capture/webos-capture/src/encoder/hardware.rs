use anyhow::{Result, ensure};
use prysm_webos_protocol::{
    Header, PixelFormat,
    h264::{FrameKind, hardware::normalize_access_unit},
};
use yuv::{BufferStoreMut, YuvBiPlanarImageMut, YuvConversionMode, YuvRange, YuvStandardMatrix};

#[allow(unsafe_code)]
#[path = "gst.rs"]
mod gst;

pub(crate) struct Hardware {
    pipeline: gst::Pipeline,
    width: u32,
    height: u32,
    stride: usize,
    pixels: Vec<u8>,
    encoded: Vec<u8>,
    first: bool,
}
impl Hardware {
    pub(super) fn new(header: Header, fps: u8, bitrate: u32) -> Result<Self> {
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
            stride,
            pixels: vec![0; stride * header.height as usize * 3 / 2],
            encoded: Vec::new(),
            first: true,
        })
    }
    pub(super) fn encode(
        &mut self,
        header: Header,
        pixels: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<Option<FrameKind>> {
        ensure!(
            header.width == self.width && header.height == self.height,
            "hardware input dimensions changed"
        );
        prepare_nv12(header, pixels, self.stride, &mut self.pixels)?;
        if !self.pipeline.encode(&self.pixels, &mut self.encoded)? {
            return Ok(None);
        }
        let kind =
            normalize_access_unit(&self.encoded, self.width, self.height, self.first, output)?;
        self.first = false;
        Ok(Some(kind))
    }
}

fn prepare_nv12(header: Header, input: &[u8], stride: usize, output: &mut [u8]) -> Result<()> {
    let width = header.width as usize;
    let height = header.height as usize;
    ensure!(
        input.len() == header.raw_len()?
            && width.is_multiple_of(2)
            && height.is_multiple_of(2)
            && stride >= width
            && stride <= 1920
            && output.len() == stride * height * 3 / 2,
        "invalid NV12 conversion layout"
    );
    let (y, uv) = output.split_at_mut(stride * height);
    y.fill(16);
    uv.fill(128);
    if header.format == PixelFormat::Rgb {
        let mut image = YuvBiPlanarImageMut {
            y_plane: BufferStoreMut::Borrowed(y),
            y_stride: u32::try_from(stride)?,
            uv_plane: BufferStoreMut::Borrowed(uv),
            uv_stride: u32::try_from(stride)?,
            width: header.width,
            height: header.height,
        };
        yuv::rgb_to_yuv_nv12(
            &mut image,
            input,
            header.width * 3,
            YuvRange::Limited,
            YuvStandardMatrix::Bt709,
            YuvConversionMode::Balanced,
        )?;
    } else {
        for row in 0..height {
            y[row * stride..row * stride + width]
                .copy_from_slice(&input[row * width..(row + 1) * width]);
        }
        let input = &input[width * height..];
        for row in 0..height / 2 {
            for col in 0..width {
                uv[row * stride + col] = if header.format == PixelFormat::Nv16 {
                    u8::try_from(
                        (u16::from(input[row * 2 * width + col])
                            + u16::from(input[(row * 2 + 1) * width + col]))
                        .div_ceil(2),
                    )?
                } else {
                    input[row * width + col]
                };
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nv16_chroma_rows_are_averaged_with_rounding() {
        let header = Header {
            width: 2,
            height: 2,
            format: PixelFormat::Nv16,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 8,
        };
        let mut output = vec![0; 192 * 3];
        prepare_nv12(header, &[16, 16, 16, 16, 10, 12, 11, 15], 192, &mut output).unwrap();
        assert_eq!(&output[384..386], &[11, 14]);
    }
    #[test]
    fn converts_capture_formats_and_initializes_every_padding_byte() {
        for format in [PixelFormat::Rgb, PixelFormat::Nv12, PixelFormat::Nv16] {
            let header = Header {
                width: 64,
                height: 34,
                format,
                codec: prysm_webos_protocol::Codec::Raw,
                payload_len: 0,
            };
            let mut input = if format == PixelFormat::Rgb {
                [255, 0, 0].repeat(64 * 34)
            } else {
                [
                    vec![63; 64 * 34],
                    [102, 240].repeat((header.raw_len().unwrap() - 64 * 34) / 2),
                ]
                .concat()
            };
            let mut output = vec![255; 192 * 34 * 3 / 2];
            prepare_nv12(header, &input, 192, &mut output).unwrap();
            for row in 0..34 {
                assert!(
                    output[row * 192..row * 192 + 64]
                        .iter()
                        .all(|v| v.abs_diff(63) <= 1)
                );
                assert!(
                    output[row * 192 + 64..(row + 1) * 192]
                        .iter()
                        .all(|v| *v == 16)
                );
            }
            for row in 0..17 {
                assert!(
                    output[192 * 34 + row * 192..192 * 34 + row * 192 + 64]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .all(|v| v[0].abs_diff(102) <= 1 && v[1].abs_diff(240) <= 1)
                );
                assert!(
                    output[192 * 34 + row * 192 + 64..192 * 34 + (row + 1) * 192]
                        .iter()
                        .all(|v| *v == 128)
                );
            }
            input.pop();
            assert!(prepare_nv12(header, &input, 192, &mut output).is_err());
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
        if header.format == PixelFormat::Rgb {
            return rgb.to_vec();
        }
        let n = header.width as usize * header.height as usize;
        let mut nv12 = vec![0; n * 3 / 2];
        prepare_nv12(
            Header {
                format: PixelFormat::Rgb,
                ..header
            },
            rgb,
            header.width as usize,
            &mut nv12,
        )
        .unwrap();
        if header.format == PixelFormat::Nv12 {
            return nv12;
        }
        let mut nv16 = nv12[..n].to_vec();
        for row in nv12[n..].chunks_exact(header.width as usize) {
            nv16.extend_from_slice(row);
            nv16.extend_from_slice(row);
        }
        nv16
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
        assert!(
            encoder
                .encode(header, &input, Content::Video)
                .unwrap()
                .is_none()
        );
        assert!(encoder.hardware_active());
        let blue = [0, 0, 255].repeat(320 * 180);
        let packet = encoder
            .encode(
                Header {
                    format: PixelFormat::Rgb,
                    ..header
                },
                &blue,
                Content::Ui,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            packet.content,
            Content::Video,
            "delayed metadata must belong to the previous input"
        );
        let rgb = prysm_webos_protocol::h264::Decoder::new()
            .unwrap()
            .decode(packet.header, packet.pixels)
            .unwrap();
        assert!(
            rgb[0] > 240 && rgb[2] < 15,
            "first wire picture must be the prior red input"
        );
        let changed = Header {
            width: 640,
            height: 360,
            format: PixelFormat::Rgb,
            ..header
        };
        let green = [0, 255, 0].repeat(640 * 360);
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
        for (width, height, format) in [
            (640, 360, PixelFormat::Rgb),
            (320, 180, PixelFormat::Nv12),
            (640, 360, PixelFormat::Nv16),
        ] {
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
            let mut hardware = Hardware::new(header, 15, 2_000_000).unwrap();
            let mut decoder = prysm_webos_protocol::h264::Decoder::new().unwrap();
            let mut output = Vec::new();
            let cpu = ticks();
            let start = Instant::now();
            let mut submitted = None;
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
                let previous = submitted.replace(now);
                let kind = hardware.encode(header, &input, &mut output).unwrap();
                if index == 0 {
                    assert!(kind.is_none());
                    std::thread::sleep(Duration::from_millis(67));
                    continue;
                }
                assert!(kind.is_some());
                eprintln!(
                    "capture_to_au_us={}",
                    previous.unwrap().elapsed().as_micros()
                );
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
                        let expected = if x >= width * 3 / 4 {
                            if index <= 15 {
                                64 + (index - 1) * 5
                            } else {
                                32 + ((index - 1) % 2) * 160
                            }
                        } else {
                            pixels[offset + c]
                        };
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
                std::thread::sleep(Duration::from_millis(67).saturating_sub(now.elapsed()));
            }
            eprintln!(
                "hardware {format:?} {width}x{height} cpu_ticks={} wall_ms={} (includes receiver decode)",
                ticks() - cpu,
                start.elapsed().as_millis()
            );
            let now = Instant::now();
            drop(hardware);
            eprintln!("hardware teardown_us={}", now.elapsed().as_micros());
            let mut software = prysm_webos_protocol::h264::Encoder::new(2_000_000, 15).unwrap();
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
                std::thread::sleep(Duration::from_millis(67).saturating_sub(now.elapsed()));
            }
            eprintln!(
                "software {format:?} {width}x{height} cpu_ticks={} wall_ms={}",
                ticks() - cpu,
                start.elapsed().as_millis()
            );
        }
        metadata_and_failure_reset();
        if std::env::var_os("PRYSM_QUALIFY_REAL_CAPTURE").is_some() {
            let mut video = crate::native::Video::open(&crate::config::CaptureOptions {
                width: 640,
                height: 360,
                fps: 15,
            })
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
                    if encoder
                        .as_mut()
                        .unwrap()
                        .encode(header, &pixels, &mut output)
                        .unwrap()
                        .is_none()
                    {
                        continue;
                    }
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
