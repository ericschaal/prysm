//! A captured pixel buffer as a [`Frame`], and when it was captured: the
//! only raw-memory reads on macOS, the counterpart of `linux/stream.rs`.
use std::slice;
use std::time::Instant;

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_media::sync::{CMClock, CMClockGetHostTimeClock};
use core_media::time::{CMTime, kCMTimeFlags_Valid};
use core_video::buffer::TCVBuffer;
use core_video::image_buffer::{CVImageBufferKeys, ycbcr_matrix_get_integer_code_point_for_string};
use core_video::pixel_buffer::{
    CVPixelBuffer, kCVPixelBufferLock_ReadOnly, kCVPixelFormatType_422YpCbCr8_yuvs,
    kCVPixelFormatType_422YpCbCr8FullRange,
};
use core_video::r#return::kCVReturnSuccess;

use prysm_core::{FrameError, PayloadError};

use crate::{BufferRejection, Frame, FramePool, PixelFormat, YuvRange, YuvStandardMatrix};

/// The host time clock, the clock capture sessions stamp samples on.
fn host_clock() -> CMClock {
    // SAFETY: CMClockGetHostTimeClock takes no arguments and returns the
    // process's host clock singleton without retaining it (the Get rule);
    // the wrapper retains it for its own life and releases only that retain.
    unsafe { CMClock::wrap_under_get_rule(CMClockGetHostTimeClock()) }
}

/// When a sample with presentation time `pts` was captured. Capture sessions
/// stamp samples on the host time clock, the clock [`Instant`] reads too.
pub(super) fn captured_at(pts: CMTime) -> Option<Instant> {
    if pts.flags & kCMTimeFlags_Valid == 0 {
        return None;
    }
    let age = host_clock().get_time().subtract(pts).get_seconds();
    crate::capture_time::since(Instant::now(), age)
}

/// Copies a `yuvs` (YUYV) pixel buffer into a [`Frame`], stripping any row
/// padding so the data matches the tightly-packed layout produced by the v4l
/// capturer on Linux.
#[allow(non_upper_case_globals)] // Names come from CoreVideo.
pub(super) fn frame_from_pixel_buffer(
    pixel_buffer: &CVPixelBuffer,
    pool: &mut FramePool,
) -> Result<Frame, BufferRejection> {
    let range = match pixel_buffer.get_pixel_format() {
        kCVPixelFormatType_422YpCbCr8_yuvs => YuvRange::Limited,
        kCVPixelFormatType_422YpCbCr8FullRange => YuvRange::Full,
        other => return Err(BufferRejection::PixelFormat(other.to_be_bytes())),
    };
    let dimension = |value: usize| {
        u32::try_from(value).map_err(|_| PayloadError::Geometry(FrameError::SizeOverflow))
    };
    let width = dimension(pixel_buffer.get_width())?;
    let height = dimension(pixel_buffer.get_height())?;
    // H.273 code points; 2 is "unspecified", as is a missing attachment.
    let matrix = pixel_buffer
        .as_buffer()
        .get_attachment(&CVImageBufferKeys::YCbCrMatrix.into(), None)
        .and_then(|value| value.downcast::<CFString>())
        .map_or(2, |value| {
            ycbcr_matrix_get_integer_code_point_for_string(&value)
        });
    let matrix = match matrix {
        1 => YuvStandardMatrix::Bt709,
        2 => crate::unspecified_matrix(height),
        5 | 6 => YuvStandardMatrix::Bt601,
        7 => YuvStandardMatrix::Smpte240,
        9 => YuvStandardMatrix::Bt2020,
        other => return Err(BufferRejection::Matrix(other)),
    };
    // Packed YUYV is one plane, and without extended pixels the base address,
    // pixel (0, 0), is the first byte of the buffer's data: the data size
    // bounds the rows.
    if pixel_buffer.is_planar() || pixel_buffer.get_extended_pixels() != (0, 0, 0, 0) {
        return Err(BufferRejection::Layout);
    }
    let data_size = pixel_buffer.get_data_size();
    if isize::try_from(data_size).is_err() {
        return Err(PayloadError::Geometry(FrameError::SizeOverflow).into());
    }
    let locked = BaseAddressLock::new(pixel_buffer).ok_or(BufferRejection::Unreadable)?;
    // SAFETY: reads the address of the buffer's data, which is locked until
    // `locked` drops, after every read of `data` below.
    let base = unsafe { pixel_buffer.get_base_address() }
        .cast_const()
        .cast::<u8>();
    if base.is_null() {
        return Err(BufferRejection::Unreadable);
    }
    // SAFETY: `base` is the first byte of the locked buffer's `data_size`
    // bytes (one plane, no extended pixels), `data_size` is at most
    // `isize::MAX` (both checked above), and `data` is last used before
    // `locked` drops. AVFoundation delivers the buffer complete and the
    // session does not write a buffer it has handed over.
    let data = unsafe { slice::from_raw_parts(base, data_size) };
    let frame = pool.copy_from_strided(
        data,
        width,
        height,
        PixelFormat::YUYV { range, matrix },
        pixel_buffer.get_bytes_per_row(),
        false,
    )?;
    drop(locked);
    Ok(frame)
}

/// A pixel buffer's base address locked for reading, unlocked on drop (also
/// when a panic unwinds past it).
struct BaseAddressLock<'a>(&'a CVPixelBuffer);

impl<'a> BaseAddressLock<'a> {
    fn new(pixel_buffer: &'a CVPixelBuffer) -> Option<Self> {
        (pixel_buffer.lock_base_address(kCVPixelBufferLock_ReadOnly) == kCVReturnSuccess)
            .then(|| Self(pixel_buffer))
    }
}

impl Drop for BaseAddressLock<'_> {
    fn drop(&mut self) {
        self.0.unlock_base_address(kCVPixelBufferLock_ReadOnly);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::base::TCFType;
    use std::time::Duration;

    #[test]
    fn capture_time_comes_from_valid_host_clock_stamps() {
        assert_eq!(captured_at(CMTime::default()), None, "invalid stamp");
        for _ in 0..3 {
            // Each wrapper releases only its own retain of the singleton.
            let captured = captured_at(host_clock().get_time()).expect("a host clock stamp");
            assert!(captured.elapsed() < Duration::from_secs(1));
        }
    }

    #[test]
    fn a_buffer_that_is_not_a_yuyv_frame_is_rejected_with_its_reason() {
        use core_video::pixel_buffer::kCVPixelFormatType_32BGRA;
        let bgra = CVPixelBuffer::new(kCVPixelFormatType_32BGRA, 4, 2, None).unwrap();
        assert_eq!(
            frame_from_pixel_buffer(&bgra, &mut FramePool::default()).unwrap_err(),
            BufferRejection::PixelFormat(*b"BGRA")
        );
        let odd = CVPixelBuffer::new(kCVPixelFormatType_422YpCbCr8_yuvs, 3, 2, None).unwrap();
        assert_eq!(
            frame_from_pixel_buffer(&odd, &mut FramePool::default()).unwrap_err(),
            BufferRejection::Payload(PayloadError::Geometry(FrameError::OddYuyvWidth(3)))
        );
    }

    #[test]
    fn pixel_buffer_metadata_and_row_padding_reach_frame() {
        let buffer = CVPixelBuffer::new(kCVPixelFormatType_422YpCbCr8_yuvs, 4, 2, None).unwrap();
        let matrix = core_video::image_buffer::CVImageBufferYCbCrMatrix::ITU_R_709_2;
        let matrix: CFString = matrix.into();
        buffer.as_buffer().set_attachment(
            &CVImageBufferKeys::YCbCrMatrix.into(),
            &matrix.as_CFType(),
            core_video::buffer::kCVAttachmentMode_ShouldPropagate,
        );
        assert_eq!(buffer.lock_base_address(0), kCVReturnSuccess);
        let stride = buffer.get_bytes_per_row();
        // SAFETY: The buffer is locked and each row has space for four YUYV pixels.
        unsafe {
            let base = buffer.get_base_address().cast::<u8>();
            for row in 0..2 {
                std::ptr::copy_nonoverlapping(
                    [16, 128, 235, 128, 16, 128, 235, 128].as_ptr(),
                    base.add(row * stride),
                    8,
                );
            }
        }
        buffer.unlock_base_address(0);
        let frame = frame_from_pixel_buffer(&buffer, &mut FramePool::default()).unwrap();
        assert_eq!(
            frame.as_slice(),
            [16, 128, 235, 128, 16, 128, 235, 128].repeat(2)
        );
        assert_eq!(
            frame.format(),
            PixelFormat::YUYV {
                range: YuvRange::Limited,
                matrix: YuvStandardMatrix::Bt709
            }
        );
    }

    #[test]
    fn a_pixel_buffer_without_a_matrix_follows_its_height() {
        for (height, matrix) in [
            (2, YuvStandardMatrix::Bt601),
            (576, YuvStandardMatrix::Bt601),
            (720, YuvStandardMatrix::Bt709),
        ] {
            let buffer =
                CVPixelBuffer::new(kCVPixelFormatType_422YpCbCr8_yuvs, 4, height, None).unwrap();
            let frame = frame_from_pixel_buffer(&buffer, &mut FramePool::default()).unwrap();
            assert_eq!(
                frame.format(),
                PixelFormat::YUYV {
                    range: YuvRange::Limited,
                    matrix
                },
                "{height}"
            );
        }
    }
}
