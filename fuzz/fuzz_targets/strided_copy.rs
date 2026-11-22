//! `FramePool::copy_from_strided`: a frame comes out exactly when the
//! geometry is valid and the payload holds every row at its stride; it holds
//! those rows without padding; and a second copy through the same pool never
//! changes a frame a consumer still holds.
#![no_main]
use prysm_core::{Frame, FramePool, PixelFormat, YuvRange, YuvStandardMatrix};

/// A format from one byte, as `pixel_format_size` reads it.
fn format(code: u8) -> PixelFormat {
    let (range, matrix) = (YuvRange::Limited, YuvStandardMatrix::Bt709);
    match code % 4 {
        0 => PixelFormat::RGB24,
        1 => PixelFormat::BGR24,
        2 => PixelFormat::YUYV { range, matrix },
        _ => PixelFormat::NV12 { range, matrix },
    }
}

/// Copy `data` and check the outcome against the rules above.
fn copy(
    pool: &mut FramePool,
    data: &[u8],
    (width, height, format, stride, corrupted): (u32, u32, PixelFormat, usize, bool),
) -> Option<Frame> {
    let frame = pool
        .copy_from_strided(data, width, height, format, stride, corrupted)
        .ok();
    // Bytes per stored row and stored rows, both NV12 planes counted.
    let geometry = match format.expected_size(width, height) {
        Ok(_) => Some(match format {
            PixelFormat::NV12 { .. } => (width as usize, (height + height / 2) as usize),
            PixelFormat::YUYV { .. } => (width as usize * 2, height as usize),
            _ => (width as usize * 3, height as usize),
        }),
        _ => None,
    };
    let fits = |(row, rows): (usize, usize)| {
        stride >= row && (data.len() as u128) >= stride as u128 * (rows as u128 - 1) + row as u128
    };
    let expected = !corrupted && width != 0 && height != 0 && geometry.is_some_and(fits);
    let context = format!(
        "{format} {width}x{height} stride {stride}, {} bytes",
        data.len()
    );
    assert_eq!(frame.is_some(), expected, "{context}");
    let frame = frame?;
    let (row, _) = geometry.unwrap();
    assert_eq!(
        (frame.width(), frame.height(), frame.format()),
        (width, height, format)
    );
    assert_eq!(
        Ok(frame.len()),
        format.expected_size(width, height),
        "{context}"
    );
    for (index, got) in frame.as_slice().chunks_exact(row).enumerate() {
        assert_eq!(
            got,
            &data[index * stride..index * stride + row],
            "{context} row {index}"
        );
    }
    Some(frame)
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    // Format and corruption, width, height (u8 each, kept small so payloads
    // can hold them) and stride (u16), then the payload.
    let Some((&[code, width, height, stride_low, stride_high], payload)) =
        data.split_first_chunk::<5>()
    else {
        return;
    };
    let shape = (
        u32::from(width),
        u32::from(height),
        format(code & 0x7f),
        usize::from(u16::from_le_bytes([stride_low, stride_high])),
        code & 0x80 != 0,
    );
    let mut pool = FramePool::default();
    let Some(held) = copy(&mut pool, payload, shape) else {
        return;
    };
    let before = held.as_slice().to_vec();
    // The same shape again from different bytes: the pool must not reuse the
    // held frame's storage.
    let flipped: Vec<u8> = payload.iter().map(|byte| !byte).collect();
    let again = copy(&mut pool, &flipped, shape).expect("the same shape copies again");
    assert_eq!(held.as_slice(), before.as_slice());
    drop(held);
    drop(again);
    // Released storage is reused; the copy still holds the new rows.
    copy(&mut pool, payload, shape).expect("the same shape copies a third time");
});
