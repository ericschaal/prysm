//! The colors stream: server-sent events carrying a few colors per edge of
//! what the LEDs show, for pages that tint with them instead of showing the
//! picture.
//!
//! Demand-driven and independent of the preview: each open stream holds a
//! [`prysm_runtime::ColorsWatch`], so the session copies the written colors (never the frame)
//! only while somebody listens, and nothing is encoded but a short JSON line.
use crate::sampled::Events;
use bytes::Bytes;
use prysm_core::{ColorStrip, EdgeColors, LinearColor};
use prysm_protocol::{COLOR_SEGMENTS, ColorsEvent, LedColor, LedColors};

pub(crate) struct Colors;

impl Events for Colors {
    type Sample = EdgeColors;
    type Key = LedColors;

    fn key(&self, colors: &EdgeColors) -> LedColors {
        summary(colors)
    }

    fn idle(&self) -> Bytes {
        sse(&ColorsEvent::Idle)
    }

    async fn encode(&self, _: EdgeColors, colors: &LedColors) -> Bytes {
        sse(&ColorsEvent::Colors(colors.clone()))
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
            for &color in &strip.samples()[start..end] {
                sum += color;
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
        let mut alternating = Vec::new();
        for _ in 0..COLOR_SEGMENTS {
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
