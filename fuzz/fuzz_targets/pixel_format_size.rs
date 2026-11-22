//! `PixelFormat::expected_size` against the same sizes worked out in `u128`:
//! an accepted size is exact and allocatable, and a refusal is an odd
//! dimension, a size beyond `isize` or an intermediate beyond `usize`.
//! crates/engine/core/tests/frame_limits.rs checks the edges on 32 bits.
#![no_main]
use prysm_core::{FrameError, PixelFormat, YuvRange, YuvStandardMatrix};

/// A format from one byte, as `strided_copy` reads it.
fn format(code: u8) -> PixelFormat {
    let (range, matrix) = (YuvRange::Limited, YuvStandardMatrix::Bt709);
    match code % 4 {
        0 => PixelFormat::RGB24,
        1 => PixelFormat::BGR24,
        2 => PixelFormat::YUYV { range, matrix },
        _ => PixelFormat::NV12 { range, matrix },
    }
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some((&code, rest)) = data.split_first() else {
        return;
    };
    let Some(bytes) = rest.first_chunk::<8>() else {
        return;
    };
    let width = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let height = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let format = format(code);
    let got = format.expected_size(width, height);
    let (w, h) = (u128::from(width), u128::from(height));
    let (odd, (row, rows)) = match format {
        PixelFormat::RGB24 | PixelFormat::BGR24 => (false, (w * 3, h)),
        PixelFormat::YUYV { .. } => (!width.is_multiple_of(2), (w * 2, h)),
        PixelFormat::NV12 { .. } => (
            !width.is_multiple_of(2) || !height.is_multiple_of(2),
            (w, h + h / 2),
        ),
    };
    match got {
        Err(FrameError::OddYuyvWidth(_) | FrameError::OddNv12Dimensions { .. }) => {
            assert!(odd, "{format} {width}x{height}");
        }
        Ok(size) => {
            assert!(!odd, "{format} {width}x{height}");
            assert_eq!(size as u128, row * rows, "{format} {width}x{height}");
            assert!(isize::try_from(size).is_ok());
        }
        Err(FrameError::SizeOverflow) => {
            assert!(!odd, "{format} {width}x{height}");
            assert!(
                row * rows > isize::MAX as u128 || row.max(rows) > usize::MAX as u128,
                "{format} {width}x{height} refused"
            );
        }
        got => panic!("{format} {width}x{height}: {got:?}"),
    }
});
