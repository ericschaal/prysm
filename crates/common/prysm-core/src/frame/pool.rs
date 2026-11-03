//! Producer-owned pixel-buffer recycling and exclusive writes before publication.
use super::{Frame, FrameError, PixelFormat};
use std::sync::Arc;

/// Producer-owned storage for up to four reusable pixel buffers.
/// Consumers keep ordinary immutable [`Frame`]s. A busy buffer is never copied or
/// overwritten: if all slots are held, one slot is replaced with fresh storage.
/// The displaced storage lives until its consumers release it.
#[derive(Debug, Default)]
pub struct FramePool {
    buffers: [Option<Arc<Vec<u8>>>; 4],
    stats: FramePoolStats,
}

/// Cumulative producer-local buffer activity, including cancelled writes.
/// Counts describe buffers, not individual allocator calls. Retained bytes exclude
/// displaced buffers still held by consumers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FramePoolStats {
    pub created: u64,
    pub reused: u64,
    pub grown: u64,
    pub exhausted: u64,
    pub retained_buffers: usize,
    pub retained_bytes: usize,
}

/// Exclusive access to a validated frame buffer before publication.
/// Fill the entire slice before calling [`Self::finish`]. Reused bytes are not cleared.
/// Dropping a writer after a failed or cancelled read publishes nothing.
pub struct FrameWriter<'a> {
    data: &'a mut Arc<Vec<u8>>,
    width: u32,
    height: u32,
    format: PixelFormat,
}

impl FrameWriter<'_> {
    /// The fixed-length destination for capture, reading or conversion.
    ///
    /// # Panics
    /// Panics if the internal exclusive-ownership invariant is violated.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        Arc::get_mut(self.data).expect("writer has exclusive access to pixels")
    }

    /// Publish the completed pixels without copying or allocating.
    #[must_use]
    pub fn finish(self) -> Frame {
        Frame {
            data: Arc::clone(self.data),
            width: self.width,
            height: self.height,
            format: self.format,
        }
    }
}

impl FramePool {
    /// Read counters without scanning or cloning pixel storage.
    #[must_use]
    pub const fn stats(&self) -> FramePoolStats {
        self.stats
    }

    /// Acquire initialized storage, reusing an unshared allocation when possible.
    /// Geometry is validated before allocating. Only fixed-size formats are supported.
    ///
    /// # Panics
    /// Panics if the internal exclusive-ownership invariant is violated.
    pub fn acquire(
        &mut self,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<FrameWriter<'_>, FrameError> {
        let size = format
            .expected_size(width, height)?
            .ok_or(FrameError::VariableSizeFormat)?;
        let index = self.buffers.iter_mut().position(|slot| {
            slot.as_mut()
                .is_none_or(|data| Arc::get_mut(data).is_some())
        });
        // Replacing a busy slot releases only the pool's reference, never a consumer's.
        let index = index.unwrap_or_else(|| {
            self.stats.exhausted += 1;
            self.stats.retained_bytes -= self.buffers[0].as_ref().map_or(0, |data| data.capacity());
            self.stats.retained_buffers -= 1;
            self.buffers[0] = None;
            0
        });
        let previous_capacity = self.buffers[index]
            .as_ref()
            .map_or(0, |data| data.capacity());
        if self.buffers[index].is_none() {
            self.stats.created += 1;
            self.stats.retained_buffers += 1;
        } else if previous_capacity < size {
            self.stats.grown += 1;
        } else {
            self.stats.reused += 1;
        }
        let data = self.buffers[index].get_or_insert_with(|| Arc::new(Vec::new()));
        Arc::get_mut(data)
            .expect("selected buffer is unshared")
            .resize(size, 0);
        self.stats.retained_bytes += data.capacity() - previous_capacity;
        Ok(FrameWriter {
            data,
            width,
            height,
            format,
        })
    }

    /// Validate a capture payload and remove row padding before constructing a frame.
    #[must_use]
    pub fn copy_from_strided(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
        stride: usize,
        corrupted: bool,
    ) -> Option<Frame> {
        if corrupted || width == 0 || height == 0 {
            return None;
        }
        format.expected_size(width, height).ok()??;
        let row_size = (width as usize).checked_mul(format.bytes_per_pixel()?)?;
        let required = stride
            .checked_mul(height as usize - 1)?
            .checked_add(row_size)?;
        if stride < row_size || data.len() < required {
            return None;
        }
        let mut frame = self.acquire(width, height, format).ok()?;
        if stride == row_size {
            frame.as_mut_slice().copy_from_slice(&data[..required]);
            return Some(frame.finish());
        }
        for (row, target) in frame.as_mut_slice().chunks_exact_mut(row_size).enumerate() {
            target.copy_from_slice(&data[row * stride..row * stride + row_size]);
        }
        Some(frame.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{YuvRange, YuvStandardMatrix};

    fn filled(pool: &mut FramePool, value: u8) -> Frame {
        let mut writer = pool.acquire(4, 2, PixelFormat::RGB24).unwrap();
        writer.as_mut_slice().fill(value);
        writer.finish()
    }

    #[test]
    fn statistics_distinguish_reuse_growth_and_exhaustion() {
        let mut pool = FramePool::default();
        assert!(
            pool.acquire(u32::MAX, u32::MAX, PixelFormat::RGB24)
                .is_err()
        );
        assert_eq!(pool.stats(), FramePoolStats::default());
        drop(filled(&mut pool, 1));
        drop(filled(&mut pool, 2));
        let _ = pool.acquire(64, 32, PixelFormat::RGB24).unwrap();
        assert_eq!(
            (
                pool.stats().created,
                pool.stats().reused,
                pool.stats().grown
            ),
            (1, 1, 1)
        );
        let held: Vec<_> = (0..5).map(|value| filled(&mut pool, value)).collect();
        let stats = pool.stats();
        assert_eq!(stats.exhausted, 1);
        assert_eq!(stats.created, 5);
        assert_eq!(stats.retained_buffers, 4);
        assert_eq!(
            stats.retained_bytes,
            pool.buffers
                .iter()
                .flatten()
                .map(|buffer| buffer.capacity())
                .sum()
        );
        assert!(held.iter().all(|frame| frame.len() == 24));
    }

    #[test]
    fn pool_reuses_storage_only_after_every_consumer_releases_it() {
        let mut pool = FramePool::default();
        let first = filled(&mut pool, 1);
        let pixels = first.as_slice().as_ptr();
        let allocation = Arc::as_ptr(&first.data);
        let retained = first.clone();
        drop(first);
        let second = filled(&mut pool, 2);
        assert_ne!(second.as_slice().as_ptr(), pixels);
        assert_eq!(retained.as_slice(), &[1; 24]);
        std::thread::spawn(move || drop(retained)).join().unwrap();
        let third = filled(&mut pool, 3);
        assert_eq!(third.as_slice().as_ptr(), pixels);
        assert_eq!(Arc::as_ptr(&third.data), allocation);
        assert_eq!(second.as_slice(), &[2; 24]);
    }

    #[test]
    fn saturation_keeps_all_published_frames_intact_and_cache_bounded() {
        let mut pool = FramePool::default();
        let held: Vec<_> = (0..20).map(|value| filled(&mut pool, value)).collect();
        assert_eq!(pool.buffers.iter().flatten().count(), 4);
        for (value, frame) in (0..20).zip(&held) {
            assert_eq!(frame.as_slice(), &[value; 24]);
        }
        drop(held);
        let frame = filled(&mut pool, 42);
        let pixels = frame.as_slice().as_ptr();
        drop(frame);
        let frame = filled(&mut pool, 43);
        assert_eq!(frame.as_slice().as_ptr(), pixels);
        drop(pool);
        assert_eq!(frame.as_slice(), &[43; 24]);
    }

    #[test]
    fn failed_write_resize_and_format_changes_do_not_publish_stale_pixels() {
        let mut pool = FramePool::default();
        let retained = filled(&mut pool, 1);
        {
            let mut incomplete = pool.acquire(4, 2, PixelFormat::RGB24).unwrap();
            incomplete.as_mut_slice()[..2].fill(99);
        }
        let mut resized = pool.acquire(8, 2, PixelFormat::BGR24).unwrap();
        resized.as_mut_slice().fill(2);
        let resized = resized.finish();
        assert_eq!(resized.as_slice(), &[2; 48]);
        assert_eq!(resized.format(), PixelFormat::BGR24);
        assert_eq!(retained.as_slice(), &[1; 24]);
        drop(resized);
        let mut smaller = pool.acquire(2, 2, PixelFormat::RGB24).unwrap();
        smaller.as_mut_slice().fill(3);
        assert_eq!(smaller.finish().as_slice(), &[3; 12]);
    }

    #[test]
    fn pool_rejects_invalid_geometry_before_allocating() {
        let mut pool = FramePool::default();
        assert!(
            pool.acquire(u32::MAX, u32::MAX, PixelFormat::RGB24)
                .is_err()
        );
        assert!(pool.acquire(2, 2, PixelFormat::MJPEG).is_err());
        let format = PixelFormat::YUYV {
            range: YuvRange::Full,
            matrix: YuvStandardMatrix::Bt709,
        };
        assert!(pool.acquire(3, 2, format).is_err());
        assert!(pool.buffers.iter().all(Option::is_none));
    }

    #[test]
    fn strided_capture_reuses_packed_storage_and_validates_each_payload() {
        let mut pool = FramePool::default();
        let frame = pool
            .copy_from_strided(&[1, 2, 3, 99, 4, 5, 6], 1, 2, PixelFormat::RGB24, 4, false)
            .unwrap();
        let pixels = frame.as_slice().as_ptr();
        assert_eq!(frame.as_slice(), &[1, 2, 3, 4, 5, 6]);
        drop(frame);
        assert!(
            pool.copy_from_strided(&[7; 6], 1, 2, PixelFormat::RGB24, 4, false)
                .is_none()
        );
        let frame = pool
            .copy_from_strided(&[8; 6], 1, 2, PixelFormat::RGB24, 3, false)
            .unwrap();
        assert_eq!(frame.as_slice().as_ptr(), pixels);
        assert_eq!(frame.as_slice(), &[8; 6]);
    }
}
