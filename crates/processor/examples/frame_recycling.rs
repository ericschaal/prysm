//! Compare fresh and recycled capture buffers, with and without processing.
//! Run: `cargo run --release -p prysm-processor --example frame_recycling`
//! This measures in-memory capture copies, not device, decoder or network latency.
// The counting allocator wraps `System`; the crate denies unsafe code elsewhere.
#![allow(unsafe_code)]
use prysm_core::{
    Config, Frame, FramePool, PixelFormat, SampleDensity, YuvRange, YuvStandardMatrix,
};
use prysm_processor::PrysmProcessor;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

struct CountedAllocator;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
fn count(bytes: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    BYTES.fetch_add(bytes, Ordering::Relaxed);
}
// SAFETY: allocation operations and their original arguments are delegated to System.
unsafe impl GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: the caller supplies a valid allocation layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: the caller supplies a valid allocation layout.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(size);
        // SAFETY: pointer, layout and size retain the caller's allocator contract.
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: this pointer was allocated by System with the supplied layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountedAllocator = CountedAllocator;

#[allow(
    clippy::cast_precision_loss,
    reason = "small benchmark counters converted for reporting"
)]
fn measure(width: u32, height: u32, format: PixelFormat, processing: bool, recycle: bool) -> u64 {
    const FRAMES: usize = 120;
    let size = format.expected_size(width, height).unwrap().unwrap();
    let source: Vec<_> = (0..size).map(|i| i.to_le_bytes()[0]).collect();
    let mut pool = FramePool::default();
    let mut processor = PrysmProcessor::new(&Config {
        sample_density: SampleDensity(128),
        remove_black_bars: false,
        ..Config::default()
    });
    let mut latest = None;
    let mut now = Instant::now();
    let mut step = || {
        let frame = if recycle {
            let mut writer = pool.acquire(width, height, format).unwrap();
            writer.as_mut_slice().copy_from_slice(black_box(&source));
            writer.finish()
        } else {
            Frame::new(black_box(source.as_slice()).to_vec(), width, height, format).unwrap()
        };
        let checksum = if processing {
            now += Duration::from_millis(33);
            let colors = black_box(processor.process_frame_at(frame.clone(), now));
            u64::from(colors.top.sample_at(0.5).to_srgb().r)
        } else {
            u64::from(black_box(frame.as_slice())[size / 2])
        };
        // A latest-frame observer retains the previous frame until publication.
        latest = Some(frame);
        checksum
    };
    for _ in 0..8 {
        black_box(step());
    }
    ALLOCATIONS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    let started = Instant::now();
    let mut checksum = 0;
    for _ in 0..FRAMES {
        checksum += black_box(step());
    }
    let elapsed = started.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    if recycle && !processing {
        assert_eq!(
            allocations, 0,
            "steady-state capture must reuse both pixel and Arc storage"
        );
    }
    println!(
        "{width}x{height} {format} {} {}: {elapsed:.3} ms/frame, {:.1} allocations/frame, {:.0} allocated bytes/frame",
        if processing { "copy+process" } else { "copy" },
        if recycle { "recycled" } else { "fresh" },
        allocations as f64 / FRAMES as f64,
        bytes as f64 / FRAMES as f64
    );
    checksum
}
fn main() {
    for (width, height) in [(1280, 720), (1920, 1080)] {
        for format in [
            PixelFormat::RGB24,
            PixelFormat::YUYV {
                range: YuvRange::Full,
                matrix: YuvStandardMatrix::Bt709,
            },
        ] {
            for processing in [false, true] {
                let fresh = measure(width, height, format, processing, false);
                let recycled = measure(width, height, format, processing, true);
                assert_eq!(fresh, recycled, "recycling changed output");
            }
        }
    }
}
