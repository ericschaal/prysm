//! `PixelFormat::expected_size` at the edges of `usize`, against a `u128`
//! reference: the TV is 32-bit, where `u32` dimensions can overflow `usize`.
//! Cheap enough for Miri, which runs it for armv7 as well (ci.yml `miri`):
//! `cargo +nightly miri test -p prysm-core --test frame_limits --target armv7-unknown-linux-gnueabi`.
use prysm_core::{FrameError, PixelFormat, YuvRange, YuvStandardMatrix};

const YUYV: PixelFormat = PixelFormat::YUYV {
    range: YuvRange::Limited,
    matrix: YuvStandardMatrix::Bt709,
};
const NV12: PixelFormat = PixelFormat::NV12 {
    range: YuvRange::Limited,
    matrix: YuvStandardMatrix::Bt709,
};

/// The exact size and the widest intermediate `expected_size` needs: the
/// bytes of one stored row, or the number of stored rows.
fn reference(format: PixelFormat, width: u32, height: u32) -> (u128, u128) {
    let (w, h) = (u128::from(width), u128::from(height));
    let (row, rows) = match format {
        PixelFormat::RGB24 | PixelFormat::BGR24 => (w * 3, h),
        PixelFormat::YUYV { .. } => (w * 2, h),
        PixelFormat::NV12 { .. } => (w, h + h / 2),
    };
    (row * rows, row.max(rows))
}

/// An accepted size is exact and allocatable; a refusal is an odd dimension,
/// a size beyond `isize`, or an intermediate beyond `usize`.
fn check(format: PixelFormat, width: u32, height: u32) {
    let got = format.expected_size(width, height);
    let odd = match format {
        PixelFormat::YUYV { .. } => !width.is_multiple_of(2),
        PixelFormat::NV12 { .. } => !width.is_multiple_of(2) || !height.is_multiple_of(2),
        _ => false,
    };
    let context = format!("{format} {width}x{height}: {got:?}");
    match (got, reference(format, width, height)) {
        (Err(FrameError::OddYuyvWidth(_) | FrameError::OddNv12Dimensions { .. }), _) => {
            assert!(odd, "{context}");
        }
        (Ok(size), (exact, _)) => {
            assert!(!odd, "{context}");
            assert_eq!(size as u128, exact, "{context}");
            assert!(isize::try_from(size).is_ok(), "{context}");
        }
        (Err(FrameError::SizeOverflow), (exact, widest)) => {
            assert!(!odd, "{context}");
            assert!(
                exact > isize::MAX as u128 || widest > usize::MAX as u128,
                "{context}"
            );
        }
        _ => panic!("{context}"),
    }
}

/// Around 2^16, isize::MAX / 3, 2^32 / 1.5 (NV12's row count) and u32::MAX.
const EDGES: [u32; 16] = [
    0,
    1,
    2,
    3,
    0xFFFE,
    0xFFFF,
    0x1_0000,
    0x2AAA_AAAA,
    0x2AAA_AAAB,
    0x5555_5554,
    0x5555_5556,
    0xAAAA_AAAA,
    0xAAAA_AAAC,
    u32::MAX - 2,
    u32::MAX - 1,
    u32::MAX,
];

#[test]
fn expected_size_matches_a_wide_reference_at_the_edges() {
    for format in [PixelFormat::RGB24, PixelFormat::BGR24, YUYV, NV12] {
        for width in EDGES {
            for height in EDGES {
                check(format, width, height);
            }
        }
    }
}
