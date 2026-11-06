//! Ratatui owns layout/diffing; this guard owns terminal acquisition/restoration.
use crate::{TerminalSettings, diagnostics::Info};
use crossterm::{
    cursor::{Hide, Show},
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use prysm_contract::{ControlError, output::ColorFrame};
use prysm_core::EdgeColors;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, IsTerminal, Stdout, Write};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Idle,
    Configured,
    Held,
    Streaming,
    Off,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Configured => "configured",
            Self::Held => "held",
            Self::Streaming => "streaming",
            Self::Off => "off",
        }
    }
}

pub(super) struct View {
    pub settings: Option<TerminalSettings>,
    pub colors: EdgeColors,
    pub frame: Option<ColorFrame>,
    pub admitted: Option<prysm_contract::identity::StreamEpoch>,
    pub mode: Mode,
    pub info: Info,
}
impl Default for View {
    fn default() -> Self {
        Self {
            settings: None,
            colors: EdgeColors::default(),
            frame: None,
            admitted: None,
            mode: Mode::Idle,
            info: Info::default(),
        }
    }
}

pub(super) trait Display: Send + 'static {
    fn draw(&mut self, view: &View) -> io::Result<()>;
    fn restore(&mut self) -> io::Result<()>;
}

pub(super) struct Screen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}
impl Screen {
    pub fn open() -> Result<Self, ControlError> {
        if !io::stdout().is_terminal() {
            return Err(ControlError::Unavailable(
                "terminal output requires an interactive stdout",
            ));
        }
        let mut screen = Self {
            terminal: Terminal::new(CrosstermBackend::new(io::stdout()))
                .map_err(|e| ControlError::Adapter(Box::new(e)))?,
            restored: false,
        };
        let acquired = (|| -> io::Result<()> {
            execute!(screen.terminal.backend_mut(), EnterAlternateScreen, Hide)?;
            screen.terminal.clear()
        })();
        if let Err(error) = acquired {
            let restored = screen.restore();
            return restore_result(Err(ControlError::Adapter(Box::new(error))), restored);
        }
        Ok(screen)
    }
}

pub(super) fn restore_result<T>(
    result: Result<T, ControlError>,
    restored: io::Result<()>,
) -> Result<T, ControlError> {
    match restored {
        Ok(()) => result,
        Err(error) => Err(ControlError::RecoveryRequired(Box::new(
            RestorationFailed {
                prior: result.err(),
                error,
            },
        ))),
    }
}

#[derive(Debug)]
struct RestorationFailed {
    prior: Option<ControlError>,
    error: io::Error,
}
impl std::fmt::Display for RestorationFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(prior) = &self.prior {
            write!(f, "{prior}; ")?;
        }
        write!(f, "terminal restoration failed: {}", self.error)
    }
}
impl std::error::Error for RestorationFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}
impl Display for Screen {
    fn draw(&mut self, view: &View) -> io::Result<()> {
        self.terminal.draw(|frame| crate::ui::render(frame, view))?;
        Ok(())
    }
    fn restore(&mut self) -> io::Result<()> {
        if !self.restored {
            // Attempt both even if restoring the cursor fails.
            let style = execute!(
                self.terminal.backend_mut(),
                ResetColor,
                SetAttribute(Attribute::Reset)
            );
            let cursor = execute!(self.terminal.backend_mut(), Show);
            let screen = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
            let flush = self.terminal.backend_mut().flush();
            style.and(cursor).and(screen).and(flush)?;
            self.restored = true;
        }
        Ok(())
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_acquisition_and_restoration_preserve_recovery_requirement() {
        let result: Result<(), ControlError> = restore_result(
            Err(ControlError::Adapter(Box::new(io::Error::other(
                "enter failed",
            )))),
            Err(io::Error::other("leave failed")),
        );
        let error = result.unwrap_err();
        assert_eq!(error.kind(), prysm_contract::FailureKind::RecoveryRequired);
        assert!(error.to_string().contains("enter failed"));
        assert!(error.to_string().contains("leave failed"));
    }
}
