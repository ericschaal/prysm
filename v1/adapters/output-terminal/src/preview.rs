//! Optional image presentation; it never chooses or replaces admitted LED colors.
use crate::{
    screen::{Mode, View},
    video::{Encoded, Encoder},
};
use prysm_contract::{
    identity::{FrameStamp, SettingsRevision},
    preview::PreviewFrame,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Paragraph, Wrap},
};
use ratatui_image::{Image, picker::ProtocolType};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) struct Sample {
    pub frame: PreviewFrame,
    pub received_at: Instant,
}
#[derive(Clone, Default)]
pub(super) enum Preview {
    #[default]
    Disabled,
    Waiting,
    Frame(Arc<Sample>),
    Unavailable(String),
}

#[derive(Default)]
pub(super) struct Presentation {
    encoder: Option<Encoder>,
    encoded: Option<Encoded>,
    submitted: Option<(FrameStamp, SettingsRevision, Rect)>,
    area: Option<Rect>,
    format: Option<((u16, u16), ProtocolType)>,
    error: Option<String>,
    failure: Option<String>,
    kitty: bool,
}
impl Presentation {
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Option<Rect>, view: &View) {
        if matches!(view.preview, Preview::Unavailable(_)) {
            self.clear();
            if let Some(encoder) = &self.encoder {
                encoder.request_shutdown();
            }
        }
        let Some(area) = area.filter(|area| area.width > 0 && area.height > 0) else {
            self.clear();
            return;
        };
        match &view.preview {
            Preview::Disabled => {
                self.clear();
                return;
            }
            Preview::Unavailable(reason) => {
                placeholder(frame, area, &format!("Preview unavailable: {reason}"));
                return;
            }
            Preview::Waiting => {
                placeholder(frame, area, "Preview waiting for capture");
                return;
            }
            Preview::Frame(_) => {}
        }
        if let Some(error) = &self.error {
            placeholder(frame, area, &format!("Preview unavailable: {error}"));
            return;
        }
        let Preview::Frame(sample) = &view.preview else {
            unreachable!()
        };
        let image_area = area;
        let size = Rect::new(0, 0, image_area.width, image_area.height);
        if self.area != Some(size) {
            self.clear();
            self.area = Some(size);
        }
        let (font, protocol) = encoding();
        self.kitty |= protocol == ProtocolType::Kitty;
        if self.format != Some((font, protocol)) {
            self.clear();
            self.area = Some(size);
            self.format = Some((font, protocol));
            if let Some(encoder) = &mut self.encoder
                && let Err(error) = encoder.reconfigure(font, protocol)
            {
                self.failed(error.to_string());
                return;
            }
        }
        if self.encoder.is_none() {
            let result = Encoder::new(font, protocol).map_err(|error| error.to_string());
            match result {
                Ok(encoder) => self.encoder = Some(encoder),
                Err(error) => {
                    self.failed(error);
                    return;
                }
            }
        }
        let encoder = self.encoder.as_ref().expect("preview encoder initialized");
        if let Some(encoded) = encoder.take() {
            if let Err(error) = &encoded.result {
                self.failed(error.clone());
                return;
            }
            if encoded.area == size {
                self.encoded = Some(encoded);
            }
        }
        let key = (sample.frame.stamp, sample.frame.revision, size);
        if self.submitted != Some(key) {
            encoder.submit(sample.frame.clone(), size, sample.received_at);
            self.submitted = Some(key);
        }
        if let Some(encoded) = &self.encoded {
            let protocol = encoded.result.as_ref().expect("successful preview image");
            let dimensions = protocol.area();
            let fitted = Rect::new(
                image_area.x + image_area.width.saturating_sub(dimensions.width) / 2,
                image_area.y + image_area.height.saturating_sub(dimensions.height) / 2,
                dimensions.width.min(image_area.width),
                dimensions.height.min(image_area.height),
            );
            frame.render_widget(Image::new(protocol), fitted);
            let caption = Rect::new(area.x, area.bottom().saturating_add(1), area.width, 1)
                .intersection(frame.area());
            placeholder(frame, caption, &caption_text(encoded, view));
        } else {
            placeholder(frame, area, "Preparing preview");
        }
    }
    fn clear(&mut self) {
        if let Some(encoder) = &self.encoder {
            encoder.clear();
        }
        self.encoded = None;
        self.submitted = None;
        self.area = None;
    }
    fn failed(&mut self, reason: String) {
        self.clear();
        if let Some(encoder) = &self.encoder {
            encoder.request_shutdown();
        }
        self.error = Some(reason.clone());
        self.failure = Some(reason);
    }
    pub fn failure(&mut self) -> Option<String> {
        self.failure.take()
    }
    pub fn uses_kitty(&self) -> bool {
        self.kitty
    }
    pub fn stop(&mut self) {
        self.clear();
        if let Some(mut encoder) = self.encoder.take()
            && let Err(reason) = encoder.shutdown()
        {
            self.failure = Some(reason);
        }
    }
}

fn caption_text(encoded: &Encoded, view: &View) -> String {
    let age = encoded.received_at.elapsed();
    let stale = view.mode != Mode::Streaming
        || view.admitted != Some(encoded.stamp.epoch)
        || age > Duration::from_secs(1);
    let paired = view
        .frame
        .as_ref()
        .is_some_and(|output| output.stamp == encoded.stamp && output.revision == encoded.revision);
    let status = if stale {
        "stale"
    } else if paired {
        "paired"
    } else {
        "latest source"
    };
    format!(
        "{status} · age {:.1}s · Frame {} · rev {} · epoch {}",
        age.as_secs_f32(),
        encoded.stamp.sequence,
        encoded.revision.0,
        encoded.stamp.epoch.generation
    )
}

fn placeholder(frame: &mut Frame<'_>, area: Rect, text: &str) {
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(Color::Gray)),
        area,
    );
}

fn encoding() -> ((u16, u16), ProtocolType) {
    let term = std::env::var("TERM").unwrap_or_default();
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    // The library's query helper leaves a detached stdin reader on timeout.
    // Native images need measured cell dimensions to stay inside the LED perimeter.
    let font = crossterm::terminal::window_size().ok().and_then(|size| {
        if size.columns == 0 || size.rows == 0 {
            return None;
        }
        let font = (size.width / size.columns, size.height / size.rows);
        (font.0 > 0 && font.1 > 0).then_some(font)
    });
    select_encoding(&term, &program, std::env::var_os("TMUX").is_some(), font)
}

fn select_encoding(
    term: &str,
    program: &str,
    tmux: bool,
    font: Option<(u16, u16)>,
) -> ((u16, u16), ProtocolType) {
    let fallback = ((1, 2), ProtocolType::Halfblocks);
    if tmux || term.starts_with("tmux") || term.starts_with("screen") || program == "tmux" {
        return fallback;
    }
    let Some(font) = font else {
        return fallback;
    };
    match (program, term) {
        ("iTerm.app" | "WezTerm", _) => (font, ProtocolType::Iterm2),
        ("ghostty" | "kitty", _) | (_, "xterm-kitty" | "xterm-ghostty") => {
            (font, ProtocolType::Kitty)
        }
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_contract::{
        identity::{InstanceId, SessionId, StreamEpoch},
        output::ColorFrame,
    };
    use prysm_core::{ColorStrip, EdgeColors, LinearColor, PixelFormat, ProcessedSnapshot};
    use ratatui::{Terminal, backend::TestBackend};
    use std::num::{NonZeroU64, NonZeroUsize};

    #[test]
    fn native_images_require_measured_geometry_and_direct_terminal_ownership() {
        let halfblocks = ((1, 2), ProtocolType::Halfblocks);
        assert_eq!(
            select_encoding("xterm-256color", "iTerm.app", false, None),
            halfblocks
        );
        assert_eq!(
            select_encoding("screen", "iTerm.app", false, Some((8, 16))),
            halfblocks
        );
        assert_eq!(
            select_encoding("xterm-kitty", "", true, Some((8, 16))),
            halfblocks
        );
        assert_eq!(
            select_encoding("xterm-kitty", "", false, Some((8, 16))),
            ((8, 16), ProtocolType::Kitty)
        );
        assert_eq!(
            select_encoding("xterm-256color", "iTerm.app", false, Some((8, 16))),
            ((8, 16), ProtocolType::Iterm2)
        );
    }

    #[tokio::test]
    async fn video_renders_inside_authorized_leds_and_marks_unpaired_or_held_images() {
        let epoch = StreamEpoch {
            session: SessionId {
                instance: InstanceId(NonZeroU64::MIN),
                sequence: NonZeroU64::MIN,
            },
            generation: 1,
        };
        let stamp = FrameStamp { epoch, sequence: 1 };
        let colors = EdgeColors {
            top: ColorStrip::fill(LinearColor::new(1.0, 0.0, 0.0), NonZeroUsize::MIN),
            ..Default::default()
        };
        let sample = Arc::new(Sample {
            frame: PreviewFrame {
                stamp,
                revision: SettingsRevision(1),
                snapshot: ProcessedSnapshot {
                    frame: prysm_core::Frame::new(
                        [0, 255, 0].repeat(32 * 16),
                        32,
                        16,
                        PixelFormat::RGB24,
                    )
                    .unwrap(),
                    viewport: None,
                    colors: EdgeColors::default(),
                },
            },
            received_at: Instant::now(),
        });
        let mut view = View {
            settings: Some(crate::TerminalSettings { leds: [1; 4] }),
            colors: colors.clone(),
            mode: Mode::Streaming,
            admitted: Some(epoch),
            frame: Some(ColorFrame {
                stamp,
                revision: SettingsRevision(1),
                colors,
            }),
            preview: Preview::Frame(sample),
            ..Default::default()
        };

        let mut presentation = Presentation {
            encoder: Some(Encoder::new((1, 2), ProtocolType::Halfblocks).unwrap()),
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
        let mut image_area = None;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                terminal
                    .draw(|frame| {
                        image_area = crate::ui::render(frame, &view);
                        presentation.render(frame, image_area, &view);
                    })
                    .unwrap();
                if presentation.encoded.is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let buffer = terminal.backend().buffer();
        let area = image_area.unwrap();
        assert_eq!(buffer[(area.x, area.y - 1)].bg, Color::Rgb(255, 0, 0));
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let cell = &buffer[(x, y)];
                assert!(
                    cell.fg == Color::Rgb(0, 255, 0) || cell.bg == Color::Rgb(0, 255, 0),
                    "unfilled preview cell {x},{y}"
                );
            }
        }
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.fg == Color::Rgb(0, 255, 0) || cell.bg == Color::Rgb(0, 255, 0))
        );
        let encoded = presentation.encoded.as_ref().unwrap();
        assert!(caption_text(encoded, &view).starts_with("paired"));
        view.frame.as_mut().unwrap().revision = SettingsRevision(2);
        assert!(caption_text(encoded, &view).starts_with("latest source"));
        view.mode = Mode::Held;
        view.admitted = None;
        view.colors = EdgeColors::default();
        terminal
            .draw(|frame| {
                let area = crate::ui::render(frame, &view);
                presentation.render(frame, area, &view);
            })
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(area.x, area.y - 1)].bg,
            Color::Rgb(0, 0, 0)
        );
        assert!(caption_text(presentation.encoded.as_ref().unwrap(), &view).starts_with("stale"));
        terminal.backend_mut().resize(1, 1);
        terminal.autoresize().unwrap();
        terminal
            .draw(|frame| {
                let area = crate::ui::render(frame, &view);
                presentation.render(frame, area, &view);
            })
            .unwrap();
        assert!(presentation.encoded.is_none());
        presentation.stop();
    }
}
