//! Video-file playback through an FFmpeg subprocess as a blocking
//! [`FrameSource`], for testing without a capture device.
//!
//! - [`FileSource`]: the factory registered under kind `"file"`.
//! - [`FileSettings`]: its human-editable settings and typed validation.
//! - `source`: the running FFmpeg child, in a process group of its own;
//!   reads raw RGB24 frames from its stdout and keeps a bounded tail of its
//!   stderr for failures. `close` and drop kill the group and reap it.
//! - [`kill_remaining`]: for a host that exits without closing a session,
//!   kills the FFmpeg children still running.
//!
//! Development only: requires FFmpeg 9+ on `PATH` (its `scale` filter maps HDR
//! to sRGB), which neither the Docker image nor the Debian package ships. FFmpeg
//! paces frames by presentation time (`realtime` filter). Playback runs once:
//! at the end of the file `next_frame` returns [`Pull::Ended`](prysm_ports::Pull::Ended),
//! the session stops without a failure and lighting turns off; starting
//! lighting again replays the file. With `repeat` FFmpeg loops the file and
//! the source never ends.
mod players;
mod settings;
mod source;

pub use players::kill_remaining;
pub use settings::FileSettings;

use prysm_ports::{
    Error, FrameSource, Modes, Result, SourceFactory, ValidationError, capture_size,
};

/// Plays video files.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileSource;

impl SourceFactory for FileSource {
    const KIND: &'static str = "file";
    type Settings = FileSettings;

    fn validate(&self, settings: &FileSettings) -> Result<(), ValidationError> {
        settings.validate()
    }

    /// FFmpeg scales to any size [`FileSettings::validate`] accepts.
    fn modes(&self, _: &FileSettings) -> Modes {
        capture_size::SIZES.into()
    }

    /// Checks the file is readable and starts FFmpeg; does not wait for a frame.
    fn open(&self, settings: &FileSettings) -> Result<Box<dyn FrameSource>> {
        std::fs::File::open(&settings.path).map_err(|error| {
            Error::adapter(format!("cannot open video file `{}`", settings.path), error)
        })?;
        Ok(Box::new(source::FileFrames::spawn(
            source::ffmpeg(settings),
            settings.width,
            settings.height,
            &settings.path,
            source::Deadlines::FFMPEG,
        )?))
    }
}

#[cfg(test)]
mod tests;
