//! The preview stream: server-sent events carrying a downscaled JPEG of the
//! latest processed frame and its sampled viewport. The LED colors have their
//! own stream (`colors.rs`).
//!
//! Demand-driven: each open stream holds a [`prysm_runtime::PreviewWatch`], so the session
//! publishes previews only while somebody watches, and a frame is encoded only
//! when a stream is due to send it. Encoding runs on the blocking pool; one
//! encoded event is shared by every viewer of the same frame.
use crate::sampled::Events;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;
use prysm_core::{Frame, Viewport};
use prysm_protocol::{PreviewEvent, PreviewFrame, PreviewViewport};
use prysm_runtime::Preview;
use std::sync::{Mutex, PoisonError};
/// The JPEG's longer side, at most; smaller frames are not enlarged.
const LONGEST_SIDE: u32 = 480;
const QUALITY: u8 = 75;

/// The latest encoded preview shared by all viewers.
#[derive(Default)]
pub(crate) struct Previews {
    encoded: Mutex<Option<Encoded>>,
}

/// One preview's event, encoded once for every viewer.
struct Encoded {
    sequence: u64,
    event: Bytes,
}

impl Events for Previews {
    type Sample = Preview;
    type Key = u64;

    fn key(&self, preview: &Preview) -> u64 {
        preview.sequence
    }

    fn idle(&self) -> Bytes {
        sse(&PreviewEvent::Idle)
    }

    async fn encode(&self, preview: Preview, _: &u64) -> Bytes {
        let sequence = preview.sequence;
        if let Some(cached) = &*lock(&self.encoded)
            && cached.sequence == sequence
        {
            return cached.event.clone();
        }
        let event = tokio::task::spawn_blocking(move || sse(&event(&preview)))
            .await
            .unwrap_or_else(|_| {
                sse(&PreviewEvent::Unavailable {
                    message: "preview encoding failed unexpectedly".into(),
                })
            });
        let mut cached = lock(&self.encoded);
        if cached
            .as_ref()
            .is_none_or(|newest| newest.sequence < sequence)
        {
            *cached = Some(Encoded {
                sequence,
                event: event.clone(),
            });
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
        jpeg: STANDARD.encode(jpeg),
    })
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
    let longest = source_width.max(source_height).max(LONGEST_SIDE);
    let width = scale(LONGEST_SIDE, source_width, longest).max(1);
    let height = scale(LONGEST_SIDE, source_height, longest).max(1);
    let full = Viewport::full_frame(source_width, source_height);
    let rgb = prysm_runtime::sample_rgb(frame, full, width, height)?.into_flattened();
    Ok(Rgb { width, height, rgb })
}

/// `value * numerator / denominator` without overflow; below `numerator`
/// whenever `value < denominator`.
fn scale(value: u32, numerator: u32, denominator: u32) -> u32 {
    let scaled = u64::from(value) * u64::from(numerator) / u64::from(denominator);
    u32::try_from(scaled).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{EdgeColors, PixelFormat};

    fn preview(frame: Frame) -> Preview {
        Preview {
            viewport: Viewport {
                x: 0,
                y: 140,
                width: frame.width(),
                height: 800,
            },
            frame,
            colors: EdgeColors::default(),
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
    fn small_and_portrait_frames_keep_their_shape() {
        let small = downscale(&Frame::dummy(16, 9).unwrap()).unwrap();
        assert_eq!((small.width, small.height), (16, 9));
        let portrait = downscale(&Frame::dummy(600, 1200).unwrap()).unwrap();
        assert_eq!((portrait.width, portrait.height), (240, 480));
        let sliver = downscale(&Frame::dummy(4000, 1).unwrap()).unwrap();
        assert_eq!((sliver.width, sliver.height), (480, 1));
    }

    #[test]
    fn empty_frames_are_unavailable() {
        let empty = Frame::dummy(0, 2).unwrap();
        assert!(matches!(
            event(&preview(empty)),
            PreviewEvent::Unavailable { .. }
        ));
    }
}
