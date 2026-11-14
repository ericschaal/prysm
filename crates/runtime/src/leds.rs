//! The session's hold on its output: opened at the first frame, released
//! after a quiet spell, left in the stop's idle mode at the end.
use crate::session::Observed;
use crate::stats::Stats;
use prysm_core::EdgeColors;
use prysm_ports::{IdleMode, LedOutput, Result};
use std::time::Instant;

/// Opens the session's output. Called on the session thread at the first
/// frame and at the first frame after each release, and at the end when the
/// final idle mode differs from the one the output was left in.
pub(crate) type OpenOutput = Box<dyn FnMut() -> Result<Box<dyn LedOutput>> + Send>;

pub(crate) enum Leds {
    /// Never opened by this session.
    Untouched,
    Held(Box<dyn LedOutput>),
    /// Idled in this mode and closed.
    Left(IdleMode),
    /// A release failed: the device may be in any mode.
    Unknown,
}

impl Leds {
    /// Write `colors`, opening the output first if it is not held. The write
    /// starts at `ready` (or when the output finished opening) and its time
    /// and outcome are counted. Returns when the write returned.
    pub(crate) fn write(
        &mut self,
        colors: &EdgeColors,
        ready: Instant,
        open: &mut OpenOutput,
        stats: &Stats,
    ) -> Result<Instant> {
        let began = if self.is_held() {
            ready
        } else {
            *self = Self::Held(open()?);
            stats.output_opened();
            Instant::now()
        };
        let Self::Held(output) = self else {
            unreachable!("held above")
        };
        let wrote = output.write(colors);
        let written = Instant::now();
        stats.output_write(written - began, wrote.is_ok());
        wrote.map(|()| written)
    }

    pub(crate) fn is_held(&self) -> bool {
        matches!(self, Self::Held(_))
    }

    /// What a held output last learned about its device.
    pub(crate) fn observe(&mut self) -> Observed {
        match self {
            Self::Held(output) => Observed {
                issue: output.observe(),
                brightness: output.brightness(),
            },
            Self::Untouched | Self::Left(_) | Self::Unknown => Observed::default(),
        }
    }

    /// Idle a held output in `mode` and close it.
    pub(crate) fn release(&mut self, mode: IdleMode) -> Result<()> {
        let Self::Held(output) = std::mem::replace(self, Self::Unknown) else {
            return Ok(());
        };
        output.idle(mode)?;
        *self = Self::Left(mode);
        Ok(())
    }

    /// Leave the output in `mode`: idle it if held; otherwise open it only
    /// when it may be in another mode. An untouched output stays untouched
    /// for `Off`, the state lighting started from.
    pub(crate) fn finish(mut self, mode: IdleMode, open: &mut OpenOutput) -> Result<()> {
        match self {
            Self::Held(_) => self.release(mode),
            Self::Left(left) if left == mode => Ok(()),
            Self::Untouched if mode == IdleMode::Off => Ok(()),
            Self::Untouched | Self::Left(_) | Self::Unknown => open()?.idle(mode),
        }
    }
}
