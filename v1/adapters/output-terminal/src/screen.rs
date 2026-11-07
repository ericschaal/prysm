//! Ratatui owns layout/diffing; this guard owns terminal acquisition/restoration.
use crate::{
    TerminalSettings,
    diagnostics::Info,
    preview::{Presentation, Preview},
};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event},
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{
        BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use prysm_contract::{ControlError, output::ColorFrame};
use prysm_core::EdgeColors;
use ratatui::{Terminal, backend::CrosstermBackend};
#[cfg(unix)]
use rustix::termios::{
    InputModes, LocalModes, OptionalActions, QueueSelector, SpecialCodeIndex, Termios, tcflush,
    tcgetattr, tcsetattr,
};
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
    pub preview: Preview,
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
            preview: Preview::Disabled,
        }
    }
}

pub(super) trait Display: Send + 'static {
    fn draw(&mut self, view: &View) -> io::Result<()>;
    fn restore(&mut self) -> io::Result<()>;
    fn poll_input(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn preview_failure(&mut self) -> Option<String> {
        None
    }
}

pub(super) struct Screen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
    #[cfg(unix)]
    input_mode: Option<Termios>,
    #[cfg(unix)]
    input_flags: Option<rustix::fs::OFlags>,
    preview: Presentation,
    commands: Option<crate::input::CommandLine>,
}
impl Screen {
    pub fn open(input: Option<tokio::sync::mpsc::Sender<String>>) -> Result<Self, ControlError> {
        if !io::stdout().is_terminal() {
            return Err(ControlError::Unavailable(
                "terminal output requires an interactive stdout",
            ));
        }
        if input.is_some() && !io::stdin().is_terminal() {
            return Err(ControlError::Unavailable(
                "terminal commands require an interactive stdin",
            ));
        }
        let mut screen = Self {
            terminal: Terminal::new(CrosstermBackend::new(io::stdout()))
                .map_err(|e| ControlError::Adapter(Box::new(e)))?,
            restored: false,
            #[cfg(unix)]
            input_mode: None,
            #[cfg(unix)]
            input_flags: None,
            preview: Presentation::default(),
            commands: input.map(crate::input::CommandLine::new),
        };
        let acquired = (|| -> io::Result<()> {
            #[cfg(unix)]
            {
                let original = tcgetattr(io::stdout())?;
                let mut quiet = original.clone();
                // Suppress scroll-key echo without disabling Ctrl-C signal delivery.
                quiet
                    .local_modes
                    .remove(LocalModes::ECHO | LocalModes::ECHONL);
                // An input-overflow BEL on macOS can terminate an in-flight image OSC.
                quiet.input_modes.remove(InputModes::IMAXBEL);
                if screen.commands.is_some() {
                    quiet.local_modes.remove(LocalModes::ICANON);
                    quiet.special_codes[SpecialCodeIndex::VMIN] = 1;
                    quiet.special_codes[SpecialCodeIndex::VTIME] = 0;
                }
                screen.input_mode = Some(original);
                tcsetattr(io::stdout(), OptionalActions::Now, &quiet)?;
            }
            execute!(screen.terminal.backend_mut(), EnterAlternateScreen, Hide)?;
            screen.terminal.clear()
        })();
        if let Err(error) = acquired {
            let restored = screen.restore();
            return restore_result(Err(ControlError::Adapter(Box::new(error))), restored);
        }
        Ok(screen)
    }

    fn read_commands(&mut self) -> io::Result<()> {
        let Some(commands) = &mut self.commands else {
            #[cfg(unix)]
            tcflush(io::stdout(), QueueSelector::IFlush)?;
            return Ok(());
        };
        #[cfg(unix)]
        {
            let flags = rustix::fs::fcntl_getfl(io::stdin())?;
            self.input_flags = Some(flags);
            // Crossterm retries reads on partial escape sequences, even with poll(0).
            rustix::fs::fcntl_setfl(io::stdin(), flags | rustix::fs::OFlags::NONBLOCK)?;
        }
        let result = (|| {
            // Bound input work so a paste/scroll flood cannot starve output or cleanup.
            for _ in 0..256 {
                // The poll-based backend needs a positive timeout to check its event buffer.
                if !event::poll(std::time::Duration::from_millis(1))? {
                    break;
                }
                if let Event::Key(key) = event::read()? {
                    commands.key(key);
                }
            }
            Ok(())
        })();
        // stdin/stdout can share file flags. Restore before any screen write.
        let restored = self.restore_input_flags();
        result.and(restored)
    }

    fn restore_input_flags(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        if let Some(flags) = self.input_flags {
            rustix::fs::fcntl_setfl(io::stdin(), flags)?;
            self.input_flags = None;
        }
        Ok(())
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
    fn poll_input(&mut self) -> io::Result<()> {
        self.read_commands()
    }
    fn draw(&mut self, view: &View) -> io::Result<()> {
        let preview = &mut self.preview;
        let commands = &self.commands;
        draw_frame(&mut self.terminal, |frame| {
            let area = crate::ui::render(frame, view);
            preview.render(frame, area, view);
            if let Some(commands) = commands {
                let area = frame.area();
                if area.height > 0 {
                    let footer =
                        ratatui::layout::Rect::new(area.x, area.bottom() - 1, area.width, 1);
                    frame.render_widget(
                        ratatui::widgets::Paragraph::new(commands.text(usize::from(area.width))),
                        footer,
                    );
                }
            }
        })?;
        Ok(())
    }
    fn preview_failure(&mut self) -> Option<String> {
        self.preview.failure()
    }
    fn restore(&mut self) -> io::Result<()> {
        self.preview.stop();
        if !self.restored {
            let flags = self.restore_input_flags();
            let resumed = execute!(self.terminal.backend_mut(), EndSynchronizedUpdate);
            let graphics = if self.preview.uses_kitty() {
                // Delete only the one image slot owned by this screen.
                write!(
                    self.terminal.backend_mut(),
                    "\x1b_Ga=d,d=I,i={},q=2\x1b\\",
                    crate::video::KITTY_IMAGE_ID
                )
            } else {
                Ok(())
            };
            // Attempt both even if restoring the cursor fails.
            let style = execute!(
                self.terminal.backend_mut(),
                ResetColor,
                SetAttribute(Attribute::Reset)
            );
            let cursor = execute!(self.terminal.backend_mut(), Show);
            let screen = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
            let flush = self.terminal.backend_mut().flush();
            let input = (|| -> io::Result<()> {
                #[cfg(unix)]
                if let Some(original) = &self.input_mode {
                    // Discard unused dashboard input before returning control to the shell.
                    tcsetattr(io::stdout(), OptionalActions::Flush, original)?;
                    self.input_mode = None;
                }
                Ok(())
            })();
            flags
                .and(resumed)
                .and(graphics)
                .and(style)
                .and(cursor)
                .and(screen)
                .and(flush)
                .and(input)?;
            self.restored = true;
        }
        Ok(())
    }
}

fn draw_frame<W: Write>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    render: impl FnOnce(&mut ratatui::Frame<'_>),
) -> io::Result<()> {
    execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let drawn = terminal.draw(render).map(|_| ());
    // Resume rendering even if drawing fails; restoration also retries this on unwind.
    let resumed = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    drawn.and(resumed)
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
    fn image_erasure_and_replacement_are_one_synchronized_update() {
        use ratatui::{TerminalOptions, Viewport, layout::Rect};
        use ratatui_image::{
            Image,
            protocol::{Protocol, iterm2::Iterm2},
        };
        let area = Rect::new(0, 0, 10, 5);
        let mut bytes = Vec::new();
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(&mut bytes),
            TerminalOptions {
                viewport: Viewport::Fixed(area),
            },
        )
        .unwrap();
        for color in [[255, 0, 0], [0, 255, 0]] {
            let image = image::RgbImage::from_pixel(80, 80, image::Rgb(color));
            let protocol = Protocol::ITerm2(Iterm2::new(image.into(), area, false).unwrap());
            draw_frame(&mut terminal, |frame| {
                frame.render_widget(Image::new(&protocol), area)
            })
            .unwrap();
        }
        drop(terminal);
        let output = String::from_utf8_lossy(&bytes);
        let mut syncing = false;
        let mut images = 0;
        // Model what WezTerm may present between erase and image transmission.
        for sequence in output.split('\x1b').skip(1) {
            if sequence.starts_with("[?2026h") {
                syncing = true;
            }
            if sequence.starts_with("[?2026l") {
                syncing = false;
            }
            if sequence.starts_with("[10X") || sequence.starts_with("]1337;File=") {
                assert!(syncing, "image update exposed an intermediate blank frame");
            }
            if sequence.starts_with("]1337;File=") {
                images += 1;
            }
        }
        assert_eq!(images, 2);
        assert!(!syncing);
    }

    #[test]
    fn failed_draw_still_ends_synchronized_rendering() {
        use ratatui::{TerminalOptions, Viewport, layout::Rect, widgets::Paragraph};
        #[derive(Default)]
        struct Writer {
            bytes: Vec<u8>,
            failed: bool,
        }
        impl Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if bytes == b"F" && !self.failed {
                    self.failed = true;
                    return Err(io::Error::other("draw failed"));
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let area = Rect::new(0, 0, 10, 5);
        let mut writer = Writer::default();
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(&mut writer),
            TerminalOptions {
                viewport: Viewport::Fixed(area),
            },
        )
        .unwrap();
        let error = draw_frame(&mut terminal, |frame| {
            frame.render_widget(Paragraph::new("FAIL"), area)
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "draw failed");
        drop(terminal);
        assert!(writer.bytes.starts_with(b"\x1b[?2026h"));
        assert!(writer.bytes.ends_with(b"\x1b[?2026l"));
    }

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
