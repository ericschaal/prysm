//! The preview stream: server-sent events carrying a downscaled JPEG of the
//! latest processed frame, its sampled viewport and the LED colors.
//!
//! Demand-driven: each open stream holds a [`PreviewWatch`], so the session
//! publishes previews only while somebody watches, and a frame is encoded only
//! when a stream is due to send it. Encoding runs on the blocking pool; one
//! encoded event is shared by every viewer of the same frame.
use crate::response::{Body, Response, failure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use futures::{StreamExt, stream};
use http_body_util::StreamBody;
use hyper::{StatusCode, body::Frame as BodyFrame, header};
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;
use prysm_core::{ColorStrip, Frame, PixelFormat};
use prysm_protocol::{
    FailureKind, PreviewColor, PreviewColors, PreviewEvent, PreviewFrame, PreviewViewport,
};
use prysm_runtime::{EngineHandle, Preview, PreviewWatch, Shutdown};
use std::convert::Infallible;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, Interval, MissedTickBehavior, interval};

/// Concurrent preview streams; more are refused with `429`.
const VIEWERS: usize = 4;
/// At most one event per stream per period: 8 per second.
pub(crate) const CADENCE: Duration = Duration::from_millis(125);
/// The JPEG's longer side, at most; smaller frames are not enlarged.
const LONGEST_SIDE: u32 = 480;
const QUALITY: u8 = 75;
/// Colors per edge, at most; longer strips are resampled.
const STRIP_SAMPLES: usize = 256;
/// A stream with nothing new sends a comment this often, so a vanished reader
/// is noticed by the write deadline and releases its viewer slot.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// The viewer limit and the latest encoded event, shared by all streams.
pub(crate) struct Previews {
    viewers: Arc<Semaphore>,
    encoded: Arc<Mutex<Option<(u64, Bytes)>>>,
}

impl Previews {
    pub(crate) fn new() -> Self {
        Self {
            viewers: Arc::new(Semaphore::new(VIEWERS)),
            encoded: Arc::default(),
        }
    }

    /// `200 text/event-stream` until the client leaves or the portal shuts
    /// down; `429 busy` beyond the viewer limit.
    pub(crate) fn stream(&self, engine: &EngineHandle, shutdown: &Shutdown) -> Response {
        let Ok(permit) = self.viewers.clone().try_acquire_owned() else {
            return failure(
                StatusCode::TOO_MANY_REQUESTS,
                FailureKind::Busy,
                "too many preview viewers",
            );
        };
        let mut ticks = interval(CADENCE);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let viewer = Viewer {
            watch: engine.preview(),
            encoded: self.encoded.clone(),
            ticks,
            shown: Shown::Starting,
            sent: Instant::now(),
            _permit: permit,
        };
        let shutdown = shutdown.clone();
        let body = stream::unfold(viewer, Viewer::next)
            .take_until(async move { shutdown.requested().await })
            .map(Ok::<_, Infallible>);
        let mut response = Response::new(Body::new(StreamBody::new(body)));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("text/event-stream"),
        );
        response
    }
}

#[derive(PartialEq, Eq)]
enum Shown {
    /// Just opened: the session starts publishing only once this stream
    /// registered, so the first tick may precede the first preview.
    Starting,
    Nothing,
    Idle,
    Sequence(u64),
}

/// One open stream. Holds at most one preview and one encoded event.
struct Viewer {
    watch: PreviewWatch,
    encoded: Arc<Mutex<Option<(u64, Bytes)>>>,
    ticks: Interval,
    shown: Shown,
    sent: Instant,
    _permit: OwnedSemaphorePermit,
}

impl Viewer {
    /// The next event, at most one per [`CADENCE`]. The body is pulled only
    /// after the previous event was written, so a slow reader gets fewer
    /// frames instead of a queue.
    async fn next(mut self) -> Option<(BodyFrame<Bytes>, Self)> {
        loop {
            self.ticks.tick().await;
            let event = match self.watch.latest() {
                None if self.shown == Shown::Idle => None,
                None if self.shown == Shown::Starting => {
                    self.shown = Shown::Nothing;
                    None
                }
                None => {
                    self.shown = Shown::Idle;
                    Some(sse(&PreviewEvent::Idle))
                }
                Some(preview) if self.shown == Shown::Sequence(preview.sequence) => None,
                Some(preview) => {
                    self.shown = Shown::Sequence(preview.sequence);
                    Some(self.encode(preview).await)
                }
            };
            let event = match event {
                Some(event) => event,
                None if self.sent.elapsed() >= KEEPALIVE => Bytes::from_static(b":\n\n"),
                None => continue,
            };
            self.sent = Instant::now();
            return Some((BodyFrame::data(event), self));
        }
    }

    /// The event for `preview`, encoded once for every viewer.
    async fn encode(&self, preview: Preview) -> Bytes {
        let sequence = preview.sequence;
        if let Some((cached, event)) = &*lock(&self.encoded)
            && *cached == sequence
        {
            return event.clone();
        }
        let event = tokio::task::spawn_blocking(move || sse(&event(&preview)))
            .await
            .unwrap_or_else(|_| {
                sse(&PreviewEvent::Unavailable {
                    message: "preview encoding failed unexpectedly".into(),
                })
            });
        let mut cached = lock(&self.encoded);
        if cached.as_ref().is_none_or(|(newest, _)| *newest < sequence) {
            *cached = Some((sequence, event.clone()));
        }
        event
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn sse(event: &PreviewEvent) -> Bytes {
    match serde_json::to_string(event) {
        Ok(json) => Bytes::from(format!("data: {json}\n\n")),
        Err(_) => Bytes::from_static(
            b"data: {\"type\":\"unavailable\",\"message\":\"preview serialization failed\"}\n\n",
        ),
    }
}

/// Encode one preview. Blocking: the JPEG encoder runs here.
pub(crate) fn event(preview: &Preview) -> PreviewEvent {
    let frame = &preview.frame;
    let image = match downscale(frame) {
        Ok(image) => image,
        Err(message) => {
            return PreviewEvent::Unavailable {
                message: message.into(),
            };
        }
    };
    let mut jpeg = Vec::new();
    if let Err(error) = JpegEncoder::new_with_quality(&mut jpeg, QUALITY).encode(
        &image.rgb,
        image.width,
        image.height,
        ExtendedColorType::Rgb8,
    ) {
        return PreviewEvent::Unavailable {
            message: format!("cannot encode the preview: {error}"),
        };
    }
    let viewport = preview.viewport;
    let colors = &preview.colors;
    PreviewEvent::Frame(PreviewFrame {
        sequence: preview.sequence,
        source_width: frame.width(),
        source_height: frame.height(),
        width: image.width,
        height: image.height,
        viewport: PreviewViewport {
            x: viewport.x,
            y: viewport.y,
            width: viewport.width,
            height: viewport.height,
        },
        colors: PreviewColors {
            top: srgb(&colors.top),
            right: srgb(&colors.right),
            bottom: srgb(&colors.bottom),
            left: srgb(&colors.left),
        },
        jpeg: STANDARD.encode(jpeg),
    })
}

/// The colors as written, in display space, at most [`STRIP_SAMPLES`] per edge.
fn srgb(strip: &ColorStrip) -> Vec<PreviewColor> {
    let count = strip.len().min(STRIP_SAMPLES);
    (0..count)
        .filter_map(|index| strip.color_at(index, count))
        .map(|color| {
            let color = color.to_srgb();
            PreviewColor([color.r, color.g, color.b])
        })
        .collect()
}

pub(crate) struct Rgb {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// The whole frame scaled (nearest pixel) to at most [`LONGEST_SIDE`] on its
/// longer side, keeping its aspect ratio. Reads only the sampled pixels.
pub(crate) fn downscale(frame: &Frame) -> Result<Rgb, &'static str> {
    let (source_width, source_height) = (frame.width(), frame.height());
    if source_width == 0 || source_height == 0 {
        return Err("the frame is empty");
    }
    if frame.format() == PixelFormat::MJPEG {
        return Err("MJPEG frames cannot be previewed");
    }
    let longest = source_width.max(source_height).max(LONGEST_SIDE);
    let width = scale(LONGEST_SIDE, source_width, longest).max(1);
    let height = scale(LONGEST_SIDE, source_height, longest).max(1);
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for y in 0..height {
        let sy = scale(y, source_height, height);
        for x in 0..width {
            let sx = scale(x, source_width, width);
            rgb.extend_from_slice(&pixel(frame, sx, sy));
        }
    }
    Ok(Rgb { width, height, rgb })
}

/// `value * numerator / denominator` without overflow; below `numerator`
/// whenever `value < denominator`.
fn scale(value: u32, numerator: u32, denominator: u32) -> u32 {
    let scaled = u64::from(value) * u64::from(numerator) / u64::from(denominator);
    u32::try_from(scaled).unwrap_or(u32::MAX)
}

fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 3] {
    let data = frame.as_slice();
    match frame.format() {
        PixelFormat::RGB24 | PixelFormat::BGR24 => {
            let offset = (y as usize * frame.width() as usize + x as usize) * 3;
            let Some(&[a, b, c]) = data.get(offset..offset + 3) else {
                return [0; 3];
            };
            if frame.format() == PixelFormat::RGB24 {
                [a, b, c]
            } else {
                [c, b, a]
            }
        }
        PixelFormat::YUYV { range, matrix } => {
            let (r, g, b) =
                prysm_core::yuyv::yuyv_pixel_to_rgb(data, x, y, frame.width(), range, matrix);
            [r, g, b]
        }
        PixelFormat::MJPEG => [0; 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{EdgeColors, LinearColor, Viewport};

    fn preview(frame: Frame) -> Preview {
        let strip = |color| ColorStrip::new(vec![color; 600]).unwrap();
        Preview {
            viewport: Viewport {
                x: 0,
                y: 140,
                width: frame.width(),
                height: 800,
            },
            frame,
            colors: EdgeColors {
                top: strip(LinearColor::new(1.0, 0.0, 0.0)),
                right: strip(LinearColor::new(0.0, 1.0, 0.0)),
                bottom: strip(LinearColor::new(0.0, 0.0, 1.0)),
                left: strip(LinearColor::new(0.5, 0.5, 0.5)),
            },
            sequence: 3,
        }
    }

    fn frame(event: PreviewEvent) -> PreviewFrame {
        match event {
            PreviewEvent::Frame(frame) => frame,
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn encodes_a_decodable_jpeg_at_bounded_size() {
        let mut bgr = Vec::new();
        for _ in 0..(1920 * 1080) {
            bgr.extend_from_slice(&[0, 0, 255]);
        }
        let source = Frame::new(bgr, 1920, 1080, PixelFormat::BGR24).unwrap();
        let encoded = frame(event(&preview(source)));
        assert_eq!((encoded.width, encoded.height), (480, 270));
        assert_eq!((encoded.source_width, encoded.source_height), (1920, 1080));
        assert_eq!(encoded.sequence, 3);
        assert_eq!(
            encoded.viewport,
            PreviewViewport {
                x: 0,
                y: 140,
                width: 1920,
                height: 800
            }
        );
        let jpeg = STANDARD.decode(&encoded.jpeg).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xFF, 0xD9]);
        let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
            .unwrap()
            .to_rgb8();
        assert_eq!(decoded.dimensions(), (480, 270));
        let [r, g, b] = decoded.get_pixel(100, 100).0;
        assert!(r > 240 && g < 16 && b < 16, "BGR is converted: {r} {g} {b}");
    }

    #[test]
    fn colors_are_srgb_and_bounded_per_edge() {
        let encoded = frame(event(&preview(Frame::dummy(16, 9).unwrap())));
        assert_eq!(encoded.colors.top.len(), STRIP_SAMPLES);
        assert_eq!(encoded.colors.top[0], PreviewColor([255, 0, 0]));
        assert_eq!(encoded.colors.right[0], PreviewColor([0, 255, 0]));
        assert_eq!(encoded.colors.bottom[0], PreviewColor([0, 0, 255]));
        // Linear 0.5 is sRGB 188, not 128.
        assert_eq!(encoded.colors.left[0], PreviewColor([188, 188, 188]));
    }

    #[test]
    fn small_and_portrait_frames_keep_their_shape() {
        let small = downscale(&Frame::dummy(16, 9).unwrap()).unwrap();
        assert_eq!((small.width, small.height), (16, 9));
        let portrait = downscale(&Frame::dummy(600, 1200).unwrap()).unwrap();
        assert_eq!((portrait.width, portrait.height), (240, 480));
        let sliver = downscale(&Frame::dummy(4000, 1).unwrap()).unwrap();
        assert_eq!((sliver.width, sliver.height), (480, 1));
    }

    #[test]
    fn unsupported_frames_are_unavailable() {
        let jpeg = Frame::new(vec![1, 2, 3], 4, 2, PixelFormat::MJPEG).unwrap();
        assert!(matches!(
            event(&preview(jpeg)),
            PreviewEvent::Unavailable { .. }
        ));
    }
}
