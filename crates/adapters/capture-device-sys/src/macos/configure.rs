//! The device's format and frame rate, and the output's pixel settings.
use std::num::NonZeroU32;

use anyhow::Context;
use av_foundation::capture_device::{AVCaptureDevice, AVFrameRateRange};
use core_media::format_description::CMVideoFormatDescription;
use core_media::time::CMTime;
use core_video::pixel_buffer::{
    kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_422YpCbCr8_yuvs,
};
use objc2::rc::Retained;
use objc2_foundation::{NSDictionary, NSNumber, NSObject, NSString};

/// The output settings for `width`×`height` `yuvs` (YUYV) pixel buffers.
pub(super) fn video_settings(
    width: u32,
    height: u32,
) -> Retained<NSDictionary<NSString, NSObject>> {
    fn ns_object(number: Retained<NSNumber>) -> Retained<NSObject> {
        Retained::into_super(Retained::into_super(number))
    }

    // SAFETY: the kCVPixelBuffer* keys are non-null constant CFStrings that
    // CoreVideo defines for the process's life; CFString is toll-free bridged
    // to NSString, so each is a valid `&'static NSString`.
    let (pixel_format_key, width_key, height_key) = unsafe {
        (
            &*kCVPixelBufferPixelFormatTypeKey.cast::<NSString>(),
            &*kCVPixelBufferWidthKey.cast::<NSString>(),
            &*kCVPixelBufferHeightKey.cast::<NSString>(),
        )
    };

    NSDictionary::from_retained_objects(
        &[pixel_format_key, width_key, height_key],
        &[
            ns_object(NSNumber::new_u32(kCVPixelFormatType_422YpCbCr8_yuvs)),
            ns_object(NSNumber::new_u32(width)),
            ns_object(NSNumber::new_u32(height)),
        ],
    )
}

/// Sets `device` to its `width`×`height` format at `refresh_rate_hz`, locked
/// until the returned lock drops.
pub(super) fn configure_capture(
    device: &AVCaptureDevice,
    width: u32,
    height: u32,
    refresh_rate_hz: NonZeroU32,
) -> anyhow::Result<ConfigurationLock<'_>> {
    let requested_width =
        i32::try_from(width).context("capture width exceeds AVFoundation's dimensions")?;
    let requested_height =
        i32::try_from(height).context("capture height exceeds AVFoundation's dimensions")?;
    let formats = device.formats();
    let (format, duration) = formats
        .iter()
        .find_map(|format| {
            let description = format
                .format_description()
                .downcast::<CMVideoFormatDescription>()?;
            let dimensions = description.get_dimensions();
            if dimensions.width != requested_width || dimensions.height != requested_height {
                return None;
            }
            let duration = format
                .video_supported_frame_rate_ranges()
                .iter()
                .find_map(|range| frame_duration(&range, refresh_rate_hz))?;
            Some((format, duration))
        })
        .with_context(|| {
            format!("capture device does not support {width}x{height} at {refresh_rate_hz} Hz")
        })?;
    device
        .lock_for_configuration()
        .context("cannot lock capture device to set format and frame rate")?;
    let lock = ConfigurationLock(device);
    device.set_active_format(&format);
    device.set_active_video_min_frame_duration(duration);
    device.set_active_video_max_frame_duration(duration);
    Ok(lock)
}

/// A device locked for configuration. A session that starts while the lock
/// is not held replaces the format and frame rate with its own choice, so the
/// lock is kept until the session runs.
pub(super) struct ConfigurationLock<'a>(&'a AVCaptureDevice);

impl Drop for ConfigurationLock<'_> {
    fn drop(&mut self) {
        self.0.unlock_for_configuration();
    }
}

/// The frame duration at which `range` delivers `fps`: 1/fps inside the
/// range, else the range's own bound when that bound is `fps` (a capture card
/// states 30 frames per second as 30.00003).
fn frame_duration(range: &AVFrameRateRange, fps: NonZeroU32) -> Option<CMTime> {
    use crate::formats::avfoundation::{Fit, fit};
    match fit(range.min_frame_rate(), range.max_frame_rate(), fps.get())? {
        Fit::Inside => Some(CMTime::make(1, i32::try_from(fps.get()).ok()?)),
        Fit::Lowest => Some(range.max_frame_duration()),
        Fit::Highest => Some(range.min_frame_duration()),
    }
}
