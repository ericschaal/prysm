use crate::formats::Rates;
use crate::health::{Event, Health, Verdict, stall_timeout};
use crate::{
    CaptureError, CaptureSink, FramePool, PixelFormat, Result, YuvRange, YuvStandardMatrix,
};
use anyhow::Context;
use prysm_ports::{Mode, Span};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};
use v4l::buffer::Type;
mod stream;
use crate::classify::{Fault, Op, classify, failure};
use stream::{Dequeued, Stream, failed, released};
use v4l::video::Capture;
use v4l::{Device, Format, FourCC};

/// The pixel formats capture negotiates, in order of preference: YUYV is
/// smaller (2 bytes a pixel against 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Captured {
    Yuyv,
    Rgb,
    Bgr,
}

impl Captured {
    const PREFERRED: [Self; 3] = [Self::Yuyv, Self::Rgb, Self::Bgr];

    fn fourcc(self) -> FourCC {
        FourCC::new(match self {
            Self::Yuyv => b"YUYV",
            Self::Rgb => b"RGB3",
            Self::Bgr => b"BGR3",
        })
    }

    fn of(fourcc: FourCC) -> Option<Self> {
        Self::PREFERRED
            .into_iter()
            .find(|captured| captured.fourcc() == fourcc)
    }
}

/// Range and matrix from the negotiated format. A matrix the device leaves
/// unnamed follows the picture `height` ([`crate::unspecified_matrix`]).
#[allow(non_upper_case_globals)] // Names come from the generated kernel bindings.
fn yuv_colorimetry(
    colorspace: u32,
    encoding: u32,
    quantization: u32,
    height: u32,
) -> Result<(YuvRange, YuvStandardMatrix)> {
    use v4l::v4l_sys::{
        v4l2_colorspace_V4L2_COLORSPACE_470_SYSTEM_BG,
        v4l2_colorspace_V4L2_COLORSPACE_470_SYSTEM_M, v4l2_colorspace_V4L2_COLORSPACE_BT2020,
        v4l2_colorspace_V4L2_COLORSPACE_DCI_P3, v4l2_colorspace_V4L2_COLORSPACE_JPEG,
        v4l2_colorspace_V4L2_COLORSPACE_REC709, v4l2_colorspace_V4L2_COLORSPACE_SMPTE170M,
        v4l2_colorspace_V4L2_COLORSPACE_SMPTE240M, v4l2_quantization_V4L2_QUANTIZATION_DEFAULT,
        v4l2_quantization_V4L2_QUANTIZATION_FULL_RANGE,
        v4l2_quantization_V4L2_QUANTIZATION_LIM_RANGE, v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_601,
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_709, v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_BT2020,
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_DEFAULT, v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_SMPTE240M,
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_SYCC,
    };
    let matrix = match encoding {
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_DEFAULT => match colorspace {
            v4l2_colorspace_V4L2_COLORSPACE_REC709 | v4l2_colorspace_V4L2_COLORSPACE_DCI_P3 => {
                YuvStandardMatrix::Bt709
            }
            v4l2_colorspace_V4L2_COLORSPACE_SMPTE240M => YuvStandardMatrix::Smpte240,
            v4l2_colorspace_V4L2_COLORSPACE_BT2020 => YuvStandardMatrix::Bt2020,
            v4l2_colorspace_V4L2_COLORSPACE_SMPTE170M
            | v4l2_colorspace_V4L2_COLORSPACE_470_SYSTEM_M
            | v4l2_colorspace_V4L2_COLORSPACE_470_SYSTEM_BG
            | v4l2_colorspace_V4L2_COLORSPACE_JPEG => YuvStandardMatrix::Bt601,
            // Default, sRGB (what uvcvideo reports without a colour-matching
            // descriptor) and the rest name no video matrix.
            _ => crate::unspecified_matrix(height),
        },
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_601 | v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_SYCC => {
            YuvStandardMatrix::Bt601
        }
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_709 => YuvStandardMatrix::Bt709,
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_BT2020 => YuvStandardMatrix::Bt2020,
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_SMPTE240M => YuvStandardMatrix::Smpte240,
        _ => {
            return Err(CaptureError::UnsupportedFormat {
                details: format!("YUYV encoding {encoding}"),
                source: None,
            });
        }
    };
    let range = match quantization {
        v4l2_quantization_V4L2_QUANTIZATION_FULL_RANGE => YuvRange::Full,
        v4l2_quantization_V4L2_QUANTIZATION_LIM_RANGE => YuvRange::Limited,
        v4l2_quantization_V4L2_QUANTIZATION_DEFAULT => {
            if colorspace == v4l2_colorspace_V4L2_COLORSPACE_JPEG {
                YuvRange::Full
            } else {
                YuvRange::Limited
            }
        }
        _ => {
            return Err(CaptureError::UnsupportedFormat {
                details: format!("YUYV quantization {quantization}"),
                source: None,
            });
        }
    };
    Ok((range, matrix))
}

fn negotiated_colorimetry(device: &Device) -> Result<(YuvRange, YuvStandardMatrix)> {
    // v4l::Format omits ycbcr_enc, so read the complete negotiated kernel format.
    // SAFETY: all-zero bytes are a valid `v4l2_format` (integers, and a union
    // of integer arrays and raw pointers); G_FMT takes exactly this structure
    // and, for the capture type set here, fills its `pix` member, the one read.
    let format = unsafe {
        let mut format = v4l::v4l_sys::v4l2_format {
            type_: Type::VideoCapture as u32,
            ..std::mem::zeroed()
        };
        v4l::v4l2::ioctl(
            device.handle().fd(),
            v4l::v4l2::vidioc::VIDIOC_G_FMT,
            std::ptr::from_mut(&mut format).cast(),
        )
        .context("failed to read negotiated capture colorimetry")?;
        format.fmt.pix
    };
    // SAFETY: both members of this union (`ycbcr_enc`, `hsv_enc`) are `u32`,
    // so either read is defined; for a YUV format the kernel sets `ycbcr_enc`.
    let encoding = unsafe { format.__bindgen_anon_1.ycbcr_enc };
    yuv_colorimetry(
        format.colorspace,
        encoding,
        format.quantization,
        format.height,
    )
}

fn configure_frame_rate(device: &Device, refresh_rate_hz: NonZeroU32) -> anyhow::Result<()> {
    let mut params = device
        .params()
        .context("failed to read capture frame interval")?;
    anyhow::ensure!(
        params
            .capabilities
            .contains(v4l::parameters::Capabilities::TIME_PER_FRAME),
        "capture device does not support configuring its frame interval"
    );
    params.interval = v4l::Fraction::new(1, refresh_rate_hz.get());
    let actual = device
        .set_params(&params)
        .context("failed to set capture frame interval")?;
    anyhow::ensure!(
        crate::formats::v4l2::interval_is(
            actual.interval.numerator,
            actual.interval.denominator,
            refresh_rate_hz.get()
        ),
        "capture device cannot provide {refresh_rate_hz} Hz (negotiated interval: {})",
        actual.interval
    );
    Ok(())
}

/// The negotiated format and what its pixels are.
fn configure_stream(
    device: &mut Device,
    width: u32,
    height: u32,
    refresh_rate_hz: NonZeroU32,
) -> Result<(Format, PixelFormat)> {
    let mut fmt = device.format().context("failed to read capture format")?;

    fmt.width = width;
    fmt.height = height;

    let mut last_error = None;
    for wanted in Captured::PREFERRED {
        fmt.fourcc = wanted.fourcc();
        if let Err(error) = device.set_format(&fmt) {
            match classify(Op::SetFormat, &error) {
                Fault::Rejected => {
                    last_error = Some(error);
                    continue;
                }
                fault @ (Fault::Gone
                | Fault::Busy
                | Fault::Denied
                | Fault::Missing
                | Fault::QueueError
                | Fault::Signalled
                | Fault::Other) => {
                    return Err(failure(fault, error, "failed to set capture format"));
                }
            }
        }
        let format = device
            .format()
            .context("failed to read negotiated capture format")?;
        // Drivers may substitute dimensions; require the configured native size.
        if format.width != width || format.height != height {
            continue;
        }
        // A driver may also substitute the format: only those read here count.
        let pixels = match Captured::of(format.fourcc) {
            Some(Captured::Yuyv) if format.width.is_multiple_of(2) => {
                let (range, matrix) = negotiated_colorimetry(device)?;
                PixelFormat::YUYV { range, matrix }
            }
            Some(Captured::Yuyv) | None => continue,
            Some(Captured::Rgb) => PixelFormat::RGB24,
            Some(Captured::Bgr) => PixelFormat::BGR24,
        };
        tracing::info!(
            "Video format set to: {:?} {}x{} (stride: {})",
            format.fourcc,
            format.width,
            format.height,
            format.stride
        );
        configure_frame_rate(device, refresh_rate_hz)?;
        return Ok((format, pixels));
    }

    Err(CaptureError::UnsupportedFormat {
        details: format!("device must support {width}x{height} capture in YUYV, RGB3 or BGR3"),
        source: last_error,
    })
}

// The shared guard's copy of the timestamp-type bits matches the kernel's.
const _: () = assert!(
    crate::capture_time::V4L2_TIMESTAMP_MASK == v4l::v4l_sys::V4L2_BUF_FLAG_TIMESTAMP_MASK
        && crate::capture_time::V4L2_TIMESTAMP_MONOTONIC
            == v4l::v4l_sys::V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC
);

/// When `buffer` was captured, if the driver stamped it on `CLOCK_MONOTONIC`.
#[allow(
    clippy::useless_conversion,
    reason = "the kernel's time fields are c_long, narrower on 32-bit targets"
)]
fn captured_at(buffer: &v4l::v4l_sys::v4l2_buffer) -> Option<Instant> {
    let now = Instant::now();
    let monotonic = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    crate::capture_time::v4l2(
        buffer.flags,
        (
            i64::from(buffer.timestamp.tv_sec),
            i64::from(buffer.timestamp.tv_usec),
        ),
        (monotonic.tv_sec, i64::from(monotonic.tv_nsec)),
        now,
    )
}

/// Nodes that capture video (not metadata or output nodes), by number.
pub(crate) fn list_devices() -> Vec<crate::DeviceInfo> {
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };
    let mut nodes: Vec<(u32, String)> = entries
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let number = name.strip_prefix("video")?.parse().ok()?;
            Some((number, format!("/dev/{name}")))
        })
        .collect();
    nodes.sort_unstable();
    nodes
        .into_iter()
        .filter_map(|(_, path)| {
            // Opening a node and asking its capabilities starts nothing.
            let caps = Device::with_path(&path).ok()?.query_caps().ok()?;
            caps.capabilities
                .contains(v4l::capability::Flags::VIDEO_CAPTURE)
                .then_some(crate::DeviceInfo {
                    name: caps.card,
                    id: path,
                })
        })
        .collect()
}

/// Every even number.
const EVEN: Span = Span::new(0, u32::MAX, 2).expect("a span");

pub(crate) fn list_formats(path: &str) -> Option<Vec<Mode>> {
    use v4l::framesize::FrameSizeEnum;
    use v4l::v4l_sys::v4l2_frmsizetypes_V4L2_FRMSIZE_TYPE_CONTINUOUS;
    // Opening a node and asking it starts nothing.
    let device = Device::with_path(path).ok()?;
    let descriptions = device.enum_formats().ok()?;
    // `configure_frame_rate` fails on a device that cannot set its interval.
    let params = device.params().ok()?;
    if !params
        .capabilities
        .contains(v4l::parameters::Capabilities::TIME_PER_FRAME)
    {
        return Some(Vec::new());
    }
    let mut found = Vec::new();
    for (fourcc, captured) in descriptions
        .into_iter()
        .filter_map(|description| Some((description.fourcc, Captured::of(description.fourcc)?)))
    {
        let yuyv = captured == Captured::Yuyv;
        // A driver that cannot list what it captures leaves it unknown.
        for size in device.enum_framesizes(fourcc).ok()? {
            let (width, height, rates) = match size.size {
                FrameSizeEnum::Discrete(size) => (
                    Span::exactly(size.width),
                    Span::exactly(size.height),
                    rates_at(&device, fourcc, size.width, size.height)?,
                ),
                FrameSizeEnum::Stepwise(sizes) => {
                    let continuous = size.typ == v4l2_frmsizetypes_V4L2_FRMSIZE_TYPE_CONTINUOUS;
                    let step = |step: u32| if continuous { 1 } else { step };
                    let (Some(width), Some(height)) = (
                        Span::new(sizes.min_width, sizes.max_width, step(sizes.step_width)),
                        Span::new(sizes.min_height, sizes.max_height, step(sizes.step_height)),
                    ) else {
                        continue;
                    };
                    // Rates are listed per size: those at both ends of the range.
                    let rates = rates_at(&device, fourcc, width.min(), height.min())?
                        .and(&rates_at(&device, fourcc, width.max(), height.max())?);
                    (width, height, rates)
                }
            };
            // YUYV pairs pixels: its widths are even.
            let Some(width) = (if yuyv { width.and(&EVEN) } else { Some(width) }) else {
                continue;
            };
            found.extend(
                rates
                    .spans()
                    .into_iter()
                    .map(|fps| Mode::new(width, height, fps)),
            );
        }
    }
    Some(found)
}

/// The whole-number rates `device` lists for `fourcc` at `width`×`height`;
/// `None` when it cannot list them.
fn rates_at(device: &Device, fourcc: FourCC, width: u32, height: u32) -> Option<Rates> {
    use v4l::frameinterval::FrameIntervalEnum;
    use v4l::v4l_sys::v4l2_frmivaltypes_V4L2_FRMIVAL_TYPE_CONTINUOUS;
    let mut rates = Rates::default();
    for interval in device.enum_frameintervals(fourcc, width, height).ok()? {
        match interval.interval {
            FrameIntervalEnum::Discrete(fraction) => {
                rates.interval(fraction.numerator, fraction.denominator);
            }
            FrameIntervalEnum::Stepwise(range) => {
                let continuous = interval.typ == v4l2_frmivaltypes_V4L2_FRMIVAL_TYPE_CONTINUOUS;
                rates.intervals(
                    (range.min.numerator, range.min.denominator),
                    (range.max.numerator, range.max.denominator),
                    (!continuous).then_some((range.step.numerator, range.step.denominator)),
                );
            }
        }
    }
    Some(rates)
}

/// Stop the stream and start it again. STREAMOFF gives every buffer back,
/// including one the driver kept.
fn restart(stream: &mut Stream) -> Result<()> {
    stream.stop().map_err(|error| {
        failed(
            Op::Stop,
            error,
            "failed to stop capture stream to restart it",
        )
    })?;
    stream
        .start()
        .map_err(|error| failed(Op::Start, error, "failed to restart capture stream"))
}

/// Carries out `verdict`: restarts (each after its pause) for as long as
/// `health` says so, where a failed `restart` is one more interruption, and
/// a device gone ends capture. `Ok(true)` to capture on, `Ok(false)` when
/// `sink` stops meanwhile.
fn settle(
    mut verdict: Verdict,
    health: &mut Health,
    sink: &dyn CaptureSink,
    mut restart: impl FnMut() -> Result<()>,
    now: impl Fn() -> Instant,
) -> Result<bool> {
    loop {
        verdict = match verdict {
            Verdict::Continue => return Ok(true),
            Verdict::Fail(error) => return Err(error),
            Verdict::Restart { pause } if sink.should_stop(pause) => return Ok(false),
            Verdict::Restart { .. } => match restart() {
                Ok(()) => Verdict::Continue,
                Err(error @ CaptureError::Disconnected { .. }) => Verdict::Fail(error),
                Err(error) => health.on(Event::Interrupted(error), now()),
            },
        };
    }
}

/// Stream from `stream`, whose buffers hold `format` as `pixels`, into
/// `sink` until it stops or capture fails.
fn capture(
    stream: &mut Stream,
    format: &Format,
    pixels: PixelFormat,
    rate: NonZeroU32,
    sink: &dyn CaptureSink,
) -> Result<()> {
    let stopped = || sink.should_stop(Duration::ZERO);
    if stopped() {
        return Ok(());
    }
    stream
        .start()
        .map_err(|error| failed(Op::Start, error, "failed to start capture stream"))?;
    sink.ready();
    let mut pool = FramePool::default();
    let mut stats = crate::trace_stats::TraceStats::new("v4l_capture");
    let mut health = Health::new(rate);
    while !stopped() {
        let started = stats.start();
        let dequeued = stream.frame(|data, buffer| {
            let captured = captured_at(buffer);
            let corrupted = buffer.flags & v4l::v4l_sys::V4L2_BUF_FLAG_ERROR != 0;
            pool.copy_from_strided(
                data,
                format.width,
                format.height,
                pixels,
                format.stride as usize,
                corrupted,
            )
            .map(|frame| frame.with_captured(captured))
        })?;
        let event = match dequeued {
            Dequeued::Buffer(Ok(frame)) => {
                stats.record(started, frame.len(), Some(pool.stats()));
                sink.frame(frame);
                Event::Frame
            }
            Dequeued::Buffer(Err(reason)) => Event::Rejected(reason.into()),
            Dequeued::Stalled => Event::Stalled,
            Dequeued::Interrupted(error) => Event::Interrupted(error),
        };
        if !matches!(event, Event::Frame) {
            stats.failure();
        }
        let verdict = health.on(event, Instant::now());
        if !settle(verdict, &mut health, sink, || restart(stream), Instant::now)? {
            return Ok(());
        }
    }
    Ok(())
}

pub(crate) fn run(
    device_path: Option<&str>,
    width: u32,
    height: u32,
    rate: NonZeroU32,
    sink: &dyn CaptureSink,
) -> Result<()> {
    let stopped = || sink.should_stop(Duration::ZERO);
    if stopped() {
        return Ok(());
    }
    let path = device_path.unwrap_or("/dev/video0");
    let mut device = Device::with_path(path).map_err(|error| match classify(Op::Open, &error) {
        Fault::Missing => CaptureError::DeviceNotFound {
            device: Some(path.into()),
            source: Some(error),
        },
        Fault::Denied => CaptureError::Denied {
            device: path.into(),
            source: error,
        },
        fault @ (Fault::Gone
        | Fault::Busy
        | Fault::Rejected
        | Fault::QueueError
        | Fault::Signalled
        | Fault::Other) => failure(fault, error, format!("failed to open video device {path}")),
    })?;
    let (format, pixels) = configure_stream(&mut device, width, height, rate)?;
    let mut stream = Stream::new(&device, stall_timeout(rate))?;
    let result = capture(&mut stream, &format, pixels, rate, sink);
    released(stream.release(), result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::PAUSE;
    use v4l::v4l_sys::*;

    #[test]
    fn missing_device_preserves_its_io_cause() {
        let result = crate::capture(
            Some("/dev/prysm-test-missing-device"),
            640,
            360,
            30.try_into().unwrap(),
            &(std::sync::Arc::new(crate::device::Discard) as std::sync::Arc<dyn CaptureSink>),
        );
        let Err(CaptureError::DeviceNotFound {
            source: Some(source),
            ..
        }) = result
        else {
            panic!("expected missing device error")
        };
        assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
    }

    /// A sink that never stops and never waits.
    struct Running;

    impl CaptureSink for Running {
        fn ready(&self) {}
        fn frame(&self, _: crate::Frame) {}
        fn should_stop(&self, _: Duration) -> bool {
            false
        }
        fn wake(&self) {}
    }

    /// `settle` after an interruption at `start`, with restarts that give
    /// `restarts` in turn (then succeed), each a pause later than the last.
    fn settled(restarts: Vec<Result<()>>) -> (Result<bool>, usize) {
        let start = Instant::now();
        let mut health = Health::new(NonZeroU32::new(30).unwrap());
        let interruption = CaptureError::Backend(anyhow::anyhow!("EIO"));
        let verdict = health.on(Event::Interrupted(interruption), start);
        let attempts = std::cell::Cell::new(0);
        let mut restarts = restarts.into_iter();
        let result = settle(
            verdict,
            &mut health,
            &Running,
            || {
                attempts.set(attempts.get() + 1);
                restarts.next().unwrap_or(Ok(()))
            },
            || start + PAUSE * u32::try_from(attempts.get()).unwrap(),
        );
        (result, attempts.get())
    }

    fn refused() -> Result<()> {
        Err(failed(
            Op::Start,
            std::io::Error::from_raw_os_error(libc::EBUSY),
            "failed to restart capture stream",
        ))
    }

    #[test]
    fn a_failed_restart_is_retried_within_the_grace() {
        let (result, attempts) = settled(vec![refused(), refused(), Ok(())]);
        assert!(result.unwrap(), "capturing again");
        assert_eq!(attempts, 3);
        // Three seconds of refusals, a pause apart: the last one ends capture.
        let (result, attempts) = settled((0..100).map(|_| refused()).collect());
        let error = result.unwrap_err();
        assert_eq!(attempts, 30, "{error}");
        assert_eq!(
            prysm_ports::chain(&error),
            "capture device is in use by another program: failed to restart capture stream: \
             Device or resource busy (os error 16)"
        );
    }

    /// A sink that never stops and records how long each check would wait.
    #[derive(Default)]
    struct Waits(std::sync::Mutex<Vec<Duration>>);

    impl CaptureSink for Waits {
        fn ready(&self) {}
        fn frame(&self, _: crate::Frame) {}
        fn should_stop(&self, wait: Duration) -> bool {
            self.0.lock().unwrap().push(wait);
            false
        }
        fn wake(&self) {}
    }

    /// A stall already waited, so its restart does not pause; an
    /// interruption's does, so a failing device does not spin.
    #[test]
    fn only_an_interruption_pauses_before_its_restart() {
        let now = Instant::now();
        let interruption = || Event::Interrupted(CaptureError::Backend(anyhow::anyhow!("EIO")));
        for (event, pause) in [(Event::Stalled, Duration::ZERO), (interruption(), PAUSE)] {
            let mut health = Health::new(NonZeroU32::new(30).unwrap());
            let verdict = health.on(event, now);
            let sink = Waits::default();
            let restarts = std::cell::Cell::new(0);
            let result = settle(
                verdict,
                &mut health,
                &sink,
                || {
                    restarts.set(restarts.get() + 1);
                    Ok(())
                },
                || now,
            );
            assert!(result.unwrap(), "capturing again");
            assert_eq!(restarts.get(), 1);
            assert_eq!(*sink.0.lock().unwrap(), [pause]);
        }
    }

    #[test]
    fn a_device_gone_during_a_restart_ends_capture_at_once() {
        let gone = Err(failed(
            Op::Start,
            std::io::Error::from_raw_os_error(libc::ENODEV),
            "failed to restart capture stream",
        ));
        let (result, attempts) = settled(vec![refused(), gone]);
        assert!(matches!(result, Err(CaptureError::Disconnected { .. })));
        assert_eq!(attempts, 2);
    }

    #[test]
    fn the_preferred_formats_are_the_ones_listed_and_read() {
        for captured in Captured::PREFERRED {
            assert_eq!(Captured::of(captured.fourcc()), Some(captured));
        }
        assert_eq!(Captured::of(FourCC::new(b"MJPG")), None);
        assert_eq!(Captured::PREFERRED[0], Captured::Yuyv);
    }

    #[test]
    fn a_missing_device_lists_nothing_known() {
        assert_eq!(crate::list_formats("/dev/prysm-test-missing-device"), None);
    }

    #[test]
    fn default_and_explicit_color_metadata() {
        let colorimetry = |colorspace, encoding, quantization, height| {
            yuv_colorimetry(colorspace, encoding, quantization, height).unwrap()
        };
        let limited = |matrix| (YuvRange::Limited, matrix);
        // No matrix named: by picture height.
        for colorspace in [0, v4l2_colorspace_V4L2_COLORSPACE_SRGB] {
            assert_eq!(
                colorimetry(colorspace, 0, 0, 1080),
                limited(YuvStandardMatrix::Bt709)
            );
            assert_eq!(
                colorimetry(colorspace, 0, 0, 576),
                limited(YuvStandardMatrix::Bt601)
            );
        }
        // Named by the colorspace or the encoding: whatever the height.
        assert_eq!(
            colorimetry(v4l2_colorspace_V4L2_COLORSPACE_SMPTE170M, 0, 0, 1080),
            limited(YuvStandardMatrix::Bt601)
        );
        assert_eq!(
            colorimetry(v4l2_colorspace_V4L2_COLORSPACE_REC709, 0, 0, 480),
            limited(YuvStandardMatrix::Bt709)
        );
        assert_eq!(
            colorimetry(v4l2_colorspace_V4L2_COLORSPACE_JPEG, 0, 0, 1080),
            (YuvRange::Full, YuvStandardMatrix::Bt601)
        );
        assert_eq!(
            colorimetry(
                v4l2_colorspace_V4L2_COLORSPACE_SRGB,
                v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_601,
                0,
                1080
            ),
            limited(YuvStandardMatrix::Bt601)
        );
        assert_eq!(
            colorimetry(
                v4l2_colorspace_V4L2_COLORSPACE_REC709,
                v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_601,
                v4l2_quantization_V4L2_QUANTIZATION_FULL_RANGE,
                1080
            ),
            (YuvRange::Full, YuvStandardMatrix::Bt601)
        );
        assert!(matches!(
            yuv_colorimetry(
                0,
                v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_BT2020_CONST_LUM,
                0,
                1080
            ),
            Err(CaptureError::UnsupportedFormat { .. })
        ));
    }
}
