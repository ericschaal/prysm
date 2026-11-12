//! Shared by `prysm-webos` and `prysm-webos-slim` (included by path).
use prysm_capture_webos::VideoSignal;
use prysm_webos_link::Video;

/// Where video readiness goes; [`VideoSignal`] in production.
pub trait VideoFeed: Send + 'static {
    fn set(&self, video: Video);
    fn reset(&self);
}

impl VideoFeed for VideoSignal {
    fn set(&self, video: Video) {
        VideoSignal::set(
            self,
            match video {
                Video::Unknown => prysm_capture_webos::Video::Unknown,
                Video::Unavailable => prysm_capture_webos::Video::Unavailable,
                Video::Available => prysm_capture_webos::Video::Available,
            },
        );
    }
    fn reset(&self) {
        VideoSignal::reset(self);
    }
}
