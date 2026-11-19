//! Regression replay using the actual repository detector and sampler.
#![allow(dead_code)]
#[path = "../../crates/processor/src/nodes/black_bar_detector.rs"]
mod detector;
#[path = "../../crates/processor/src/frames/mod.rs"]
mod frames;
#[path = "../../crates/processor/src/nodes/spatial_sampler.rs"]
mod sampler;
use detector::BlackBarDetector;
use frames::{FrameView, Viewport};
use prysm_core::{Frame, PixelFormat, SampleDensity, YuvRange, YuvStandardMatrix};
use std::time::{Duration, Instant};

fn picture(luma: impl Fn(u32, u32) -> u8) -> Frame {
    let mut bytes = Vec::with_capacity(640 * 360 * 2);
    for y in 0..360 {
        for x in 0..640 {
            bytes.extend([luma(x, y), 128]);
        }
    }
    Frame::new(
        bytes,
        640,
        360,
        PixelFormat::YUYV {
            range: YuvRange::Limited,
            matrix: YuvStandardMatrix::Bt709,
        },
    )
    .unwrap()
}
fn boxed(top: u32, bottom: u32) -> Frame {
    picture(|_, y| {
        if y < top || y >= 360 - bottom {
            16
        } else {
            180
        }
    })
}
fn settle(d: &mut BlackBarDetector, frame: &Frame, now: Instant) -> Viewport {
    (0..=50)
        .map(|step| {
            d.process(
                FrameView::new(frame.clone()),
                now + Duration::from_millis(step * 50),
            )
            .viewport
        })
        .last()
        .unwrap()
}
fn main() {
    let origin = Instant::now();
    let mut d = BlackBarDetector::new();
    for (i, bottom, expected) in [(0, 11, (20, 11)), (3, 10, (20, 11)), (6, 11, (20, 11))] {
        let v = settle(&mut d, &boxed(20, bottom), origin + Duration::from_secs(i));
        let observed = (v.y, 360 - v.y - v.height);
        assert_eq!(observed, expected);
        println!("asymmetry: actual=(20,{bottom}), detected={observed:?}");
    }
    let mut d = BlackBarDetector::new();
    assert_eq!(settle(&mut d, &boxed(20, 20), origin).y, 20);
    let dark_edges = picture(|_, y| {
        if y < 20 || y >= 340 {
            16
        } else if y < 40 || y >= 320 {
            22
        } else {
            180
        }
    });
    let v = settle(&mut d, &dark_edges, origin + Duration::from_secs(3));
    assert_eq!(v.y, 20);
    println!(
        "dark picture: true bars=(20,20), detected=({},{}), dim picture preserved",
        v.y,
        360 - v.y - v.height
    );
    let black = picture(|_, _| 16);
    assert_eq!(settle(&mut d, &black, origin + Duration::from_secs(6)), v);
    println!("full darkness: confirmed crop preserved");
    let mut d = BlackBarDetector::new();
    assert_eq!(settle(&mut d, &boxed(48, 48), origin).y, 48);
    let subtitle = picture(|x, y| {
        if (220..420).contains(&x) && (330..337).contains(&y) {
            235
        } else if y < 48 || y >= 312 {
            16
        } else {
            180
        }
    });
    let v = settle(&mut d, &subtitle, origin + Duration::from_secs(3));
    assert_eq!((v.y, 360 - v.y - v.height), (48, 48));
    println!(
        "subtitle: true bars=(48,48), detected=({},{}), both boundaries preserved",
        v.y,
        360 - v.y - v.height
    );
    let mut s = sampler::SpatialSampler::new(SampleDensity(256), 0.2);
    let frame = boxed(20, 11);
    for (top, bottom) in [(20, 11), (10, 10)] {
        let mut view = FrameView::new(frame.clone());
        view.viewport = Viewport {
            x: 0,
            y: top,
            width: 640,
            height: 360 - top - bottom,
        };
        let colors = s.process(&view);
        let top_colors = colors.top.resample(51);
        let mean = top_colors.iter().map(|c| f64::from(c.r)).sum::<f64>() / 51.0;
        println!("same synthetic frame, crop=({top},{bottom}), mean top linear red={mean:.6}");
    }
    if let Some(path) = std::env::args().nth(1) {
        let frame = Frame::new(std::fs::read(path).unwrap(), 480, 270, PixelFormat::RGB24).unwrap();
        let mut results = Vec::new();
        for (top, bottom) in [(20, 11), (10, 10)] {
            let y = top * 270 / 360;
            let end = (360 - bottom) * 270 / 360;
            let mut view = FrameView::new(frame.clone());
            view.viewport = Viewport {
                x: 0,
                y,
                width: 480,
                height: end - y,
            };
            let colors = s.process(&view);
            results.push(colors.top.resample(51));
        }
        let mut peak = 0u8;
        let mut sum = 0u32;
        for (a, b) in results[0].iter().zip(&results[1]) {
            let a = a.to_srgb();
            let b = b.to_srgb();
            for delta in [a.r.abs_diff(b.r), a.g.abs_diff(b.g), a.b.abs_diff(b.b)] {
                peak = peak.max(delta);
                sum += u32::from(delta);
            }
        }
        println!(
            "same JPEG preview, scaled crops: top LED max sRGB channel delta={peak}/255, mean={:.3}/255; no smoothing or shadow lift",
            f64::from(sum) / 153.0
        );
    }
}
