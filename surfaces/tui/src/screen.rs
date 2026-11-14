//! The screen thread: owns the terminal, reads keys and redraws. All blocking
//! terminal I/O happens here, never on the async runtime.
use crate::command::Command;
use crate::input::{Action, Notice, Ui};
use crate::view::{self, View};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event},
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{
        BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
        disable_raw_mode, enable_raw_mode,
    },
};
use prysm_ports::{Error, Result, Snapshot};
use prysm_runtime::PreviewWatch;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};

/// Redraw interval; also the longest a stop request waits for the thread.
const REDRAW: Duration = Duration::from_millis(50);
/// Input handled per redraw, so a paste or scroll flood cannot starve drawing.
const MAX_EVENTS: usize = 64;

/// How the screen thread ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// The user asked to quit.
    Quit,
    /// The surface asked it to stop.
    Stopped,
}

/// What the screen reads and where it sends commands.
pub(crate) struct Feed {
    pub snapshot: watch::Receiver<Snapshot>,
    /// Registered for the screen's lifetime, so the session publishes previews.
    pub preview: PreviewWatch,
    pub commands: mpsc::Sender<Command>,
    pub notices: std_mpsc::Receiver<Notice>,
}

/// The running screen thread. Dropping it stops and joins the thread, so the
/// terminal is restored even if the owning task is aborted.
pub(crate) struct ScreenThread {
    stop: Arc<AtomicBool>,
    pub done: oneshot::Receiver<Result<Exit>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl ScreenThread {
    pub fn spawn(feed: Feed) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (report, done) = oneshot::channel();
        let flag = stop.clone();
        let join = std::thread::Builder::new()
            .name("prysm-tui".into())
            .spawn(move || {
                // A panic drops `report`, which the surface reports as a panic;
                // the screen's Drop restores the terminal while unwinding.
                let _ = report.send(run(feed, &flag));
            })
            .map_err(|error| Error::adapter("cannot start the terminal thread", error))?;
        Ok(Self {
            stop,
            done,
            join: Some(join),
        })
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Drop for ScreenThread {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(join) = self.join.take() {
            // Bounded by one redraw interval once the flag is set.
            let _ = join.join();
        }
    }
}

fn run(mut feed: Feed, stop: &AtomicBool) -> Result<Exit> {
    let io = |error| Error::adapter("terminal I/O failed", error);
    let mut screen = Screen::open().map_err(io)?;
    let result = render_loop(&mut screen, &mut feed, stop).map_err(io);
    let restored = screen
        .restore()
        .map_err(|error| Error::adapter("terminal restoration failed", error));
    // The loop failure happened first; restoration is retried by Drop anyway.
    result.and_then(|exit| restored.map(|()| exit))
}

fn render_loop(screen: &mut Screen, feed: &mut Feed, stop: &AtomicBool) -> io::Result<Exit> {
    let mut ui = Ui::default();
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(Exit::Stopped);
        }
        while let Ok(notice) = feed.notices.try_recv() {
            ui.notify(notice);
        }
        let snapshot = feed.snapshot.borrow().clone();
        // Not taken: other surfaces may render the same preview.
        let preview = feed.preview.latest();
        screen.draw(&View {
            snapshot: &snapshot,
            preview: preview.as_ref(),
            ui: &ui,
        })?;
        let deadline = Instant::now() + REDRAW;
        let mut wait = REDRAW;
        for _ in 0..MAX_EVENTS {
            if !event::poll(wait)? {
                break;
            }
            // Redraw right after input, draining what already arrived.
            wait = Duration::ZERO;
            let Event::Key(key) = event::read()? else {
                continue;
            };
            match ui.key(key) {
                None => {}
                Some(Action::Quit) => return Ok(Exit::Quit),
                Some(Action::Run(command)) => match feed.commands.try_send(command) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        ui.notify(Notice::error("Busy: earlier commands are still running"));
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => return Ok(Exit::Stopped),
                },
            }
            if Instant::now() >= deadline {
                break;
            }
        }
    }
}

/// Raw mode, alternate screen and hidden cursor, restored exactly once.
struct Screen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

impl Screen {
    fn open() -> io::Result<Self> {
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        let mut screen = Self {
            terminal,
            restored: false,
        };
        // From here on, Drop undoes whatever part succeeded.
        enable_raw_mode()?;
        execute!(screen.terminal.backend_mut(), EnterAlternateScreen, Hide)?;
        screen.terminal.clear()?;
        Ok(screen)
    }

    fn draw(&mut self, view: &View<'_>) -> io::Result<()> {
        draw_synchronized(&mut self.terminal, |frame| view::render(frame, view))
    }

    fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        let backend = self.terminal.backend_mut();
        // Attempt every step even if an earlier one fails.
        let resumed = execute!(backend, EndSynchronizedUpdate);
        let style = execute!(backend, ResetColor, SetAttribute(Attribute::Reset));
        let cursor = execute!(backend, Show);
        let screen = execute!(backend, LeaveAlternateScreen);
        let flushed = backend.flush();
        let raw = disable_raw_mode();
        self.restored = true;
        resumed
            .and(style)
            .and(cursor)
            .and(screen)
            .and(flushed)
            .and(raw)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Draw inside one synchronized update so supporting terminals never show a
/// half-drawn frame. The end marker is sent even if drawing fails.
fn draw_synchronized<W: Write>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    render: impl FnOnce(&mut ratatui::Frame<'_>),
) -> io::Result<()> {
    execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let drawn = terminal.draw(render).map(|_| ());
    let resumed = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    drawn.and(resumed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{TerminalOptions, Viewport, layout::Rect, widgets::Paragraph};

    #[test]
    fn failed_draw_still_ends_synchronized_rendering() {
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
        let error = draw_synchronized(&mut terminal, |frame| {
            frame.render_widget(Paragraph::new("FAIL"), area);
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "draw failed");
        drop(terminal);
        assert!(writer.bytes.starts_with(b"\x1b[?2026h"));
        assert!(writer.bytes.ends_with(b"\x1b[?2026l"));
    }
}
