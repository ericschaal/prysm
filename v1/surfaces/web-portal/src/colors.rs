//! The colors stream: server-sent events carrying a few colors per edge of
//! what the LEDs show, for pages that tint with them instead of showing the
//! picture.
//!
//! Demand-driven and independent of the preview: each open stream holds a
//! [`ColorsWatch`], so the session copies the written colors (never the frame)
//! only while somebody listens, and nothing is encoded but a short JSON line.
use crate::response::{Body, Response, failure};
use bytes::Bytes;
use futures::{StreamExt, stream};
use http_body_util::StreamBody;
use hyper::{StatusCode, body::Frame as BodyFrame, header};
use prysm_core::{ColorStrip, EdgeColors, LinearColor};
use prysm_protocol::{COLOR_SEGMENTS, ColorsEvent, FailureKind, LedColor, LedColors};
use prysm_runtime::{ColorsWatch, EngineHandle, Shutdown};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, Interval, MissedTickBehavior, interval};

/// Concurrent colors streams; more are refused with `429`.
const LISTENERS: usize = 8;
/// At most one event per stream per period: 10 per second.
pub(crate) const CADENCE: Duration = Duration::from_millis(100);
/// A stream with nothing new sends a comment this often, so a vanished reader
/// is noticed by the write deadline and releases its slot.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// The listener limit, shared by all streams.
pub(crate) struct Colors {
    listeners: Arc<Semaphore>,
}

impl Colors {
    pub(crate) fn new() -> Self {
        Self {
            listeners: Arc::new(Semaphore::new(LISTENERS)),
        }
    }

    /// `200 text/event-stream` until the client leaves or the portal shuts
    /// down; `429 busy` beyond the listener limit.
    pub(crate) fn stream(&self, engine: &EngineHandle, shutdown: &Shutdown) -> Response {
        let Ok(permit) = self.listeners.clone().try_acquire_owned() else {
            return failure(
                StatusCode::TOO_MANY_REQUESTS,
                FailureKind::Busy,
                "too many colors streams",
            );
        };
        let mut ticks = interval(CADENCE);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let listener = Listener {
            watch: engine.colors(),
            ticks,
            shown: Shown::Starting,
            sent: Instant::now(),
            _permit: permit,
        };
        let shutdown = shutdown.clone();
        let body = stream::unfold(listener, Listener::next)
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
    /// Just opened: the session publishes only once this stream registered,
    /// so the first tick may precede the first colors.
    Starting,
    Nothing,
    Idle,
    Colors(LedColors),
}

/// One open stream; remembers the last colors it sent.
struct Listener {
    watch: ColorsWatch,
    ticks: Interval,
    shown: Shown,
    sent: Instant,
    _permit: OwnedSemaphorePermit,
}

impl Listener {
    /// The next event, at most one per [`CADENCE`] and only when it differs
    /// from the last one sent. The body is pulled only after the previous
    /// event was written, so a slow reader gets fewer events, not a queue.
    async fn next(mut self) -> Option<(BodyFrame<Bytes>, Self)> {
        loop {
            self.ticks.tick().await;
            let shown = match self.watch.latest() {
                None if self.shown == Shown::Starting => Shown::Nothing,
                None => Shown::Idle,
                Some(colors) => Shown::Colors(summary(&colors)),
            };
            let event = if shown == self.shown || shown == Shown::Nothing {
                None
            } else {
                Some(match &shown {
                    Shown::Colors(colors) => sse(&ColorsEvent::Colors(colors.clone())),
                    Shown::Starting | Shown::Nothing | Shown::Idle => sse(&ColorsEvent::Idle),
                })
            };
            self.shown = shown;
            let event = match event {
                Some(event) => event,
                None if self.sent.elapsed() >= KEEPALIVE => Bytes::from_static(b":\n\n"),
                None => continue,
            };
            self.sent = Instant::now();
            return Some((BodyFrame::data(event), self));
        }
    }
}

fn sse(event: &ColorsEvent) -> Bytes {
    match serde_json::to_string(event) {
        Ok(json) => Bytes::from(format!("data: {json}\n\n")),
        Err(_) => Bytes::from_static(b"data: {\"type\":\"idle\"}\n\n"),
    }
}

/// [`COLOR_SEGMENTS`] colors per edge, in display space.
pub(crate) fn summary(colors: &EdgeColors) -> LedColors {
    LedColors {
        top: segments(&colors.top),
        right: segments(&colors.right),
        bottom: segments(&colors.bottom),
        left: segments(&colors.left),
    }
}

/// The strip cut into at most [`COLOR_SEGMENTS`] equal stretches, each
/// averaged in linear light, so one bright LED does not stand for its
/// neighbours.
fn segments(strip: &ColorStrip) -> Vec<LedColor> {
    let len = strip.len();
    let count = len.min(COLOR_SEGMENTS);
    (0..count)
        .map(|segment| {
            let (start, end) = (segment * len / count, (segment + 1) * len / count);
            let mut sum = LinearColor::black();
            for index in start..end {
                if let Some(color) = strip.color_at(index, len) {
                    sum += color;
                }
            }
            #[expect(
                clippy::cast_precision_loss,
                reason = "a strip has far fewer samples than f32's exact integer range"
            )]
            let color = (sum / (end - start) as f32).to_srgb();
            LedColor([color.r, color.g, color.b])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(colors: &[LinearColor]) -> ColorStrip {
        ColorStrip::new(colors.to_vec()).unwrap()
    }

    #[test]
    fn edges_are_averaged_into_srgb_segments() {
        let (red, blue) = (
            LinearColor::new(1.0, 0.0, 0.0),
            LinearColor::new(0.0, 0.0, 1.0),
        );
        // 16 samples: each segment averages two, red then blue.
        let mut alternating = Vec::new();
        for _ in 0..8 {
            alternating.extend([red, blue]);
        }
        let colors = EdgeColors::new(
            strip(&[red; 300]),
            strip(&alternating),
            strip(&[LinearColor::new(0.5, 0.5, 0.5)]),
            strip(&[red, blue, red]),
        );
        let summary = summary(&colors);
        assert_eq!(summary.top, vec![LedColor([255, 0, 0]); COLOR_SEGMENTS]);
        // Linear 0.5 is sRGB 188, not 128.
        assert_eq!(summary.right, vec![LedColor([188, 0, 188]); COLOR_SEGMENTS]);
        assert_eq!(summary.bottom, vec![LedColor([188, 188, 188])]);
        assert_eq!(
            summary.left,
            vec![
                LedColor([255, 0, 0]),
                LedColor([0, 0, 255]),
                LedColor([255, 0, 0])
            ],
            "short edges keep their samples"
        );
    }
}
