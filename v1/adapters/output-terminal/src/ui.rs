//! Pure presentation. No terminal I/O, lifecycle decisions, or capture access.
use crate::{
    diagnostics::{Diagnostic, Severity},
    screen::{Mode, View},
};
use prysm_core::ColorStrip;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Widget, Wrap},
};

const BACKGROUND: Color = Color::Rgb(16, 20, 28);
const TEXT: Color = Color::Rgb(225, 231, 239);
const MUTED: Color = Color::Rgb(150, 164, 183);
const BORDER: Color = Color::Rgb(64, 80, 101);
const ACCENT: Color = Color::Rgb(102, 213, 219);

pub(super) fn render(frame: &mut Frame<'_>, view: &View) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKGROUND).fg(TEXT)),
        frame.area(),
    );
    // Below this size, prioritize current host state and the newest diagnostic.
    if frame.area().width < 72 || frame.area().height < 22 {
        compact(frame, view);
        return;
    }
    let [header, host, content, events, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(if frame.area().height >= 28 { 8 } else { 6 }),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    frame.render_widget(header_line(view), header);
    frame.render_widget(
        Paragraph::new(host_line(view))
            .wrap(Wrap { trim: true })
            .block(panel(" Host ")),
        host,
    );
    let [preview, status] = Layout::horizontal([Constraint::Min(24), Constraint::Length(32)])
        .spacing(1)
        .areas(content);
    preview_panel(frame, view, preview);
    frame.render_widget(
        Paragraph::new(vec![
            section("LAST APPLICATION SNAPSHOT"),
            Line::from(fallback(&view.info.application, "Awaiting application")),
            Line::default(),
            Line::from(fallback(&view.info.capture, "Capture: unknown")),
            Line::default(),
            Line::from(fallback(&view.info.revisions, "Processing: not configured")),
        ])
        .wrap(Wrap { trim: true })
        .block(panel(" Application ")),
        status,
    );
    let block = panel(" Diagnostics · newest first ");
    let rows = usize::from(block.inner(events).height);
    let lines: Vec<_> = if view.info.messages.is_empty() {
        vec![Line::styled("No events yet", Style::default().fg(MUTED))]
    } else {
        view.info
            .messages
            .iter()
            .rev()
            .take(rows)
            .map(event_line)
            .collect()
    };
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(block),
        events,
    );
    frame.render_widget(footer_line(), footer);
}

fn compact(frame: &mut Frame<'_>, view: &View) {
    let [header, host, preview, application, event, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    frame.render_widget(header_line(view), header);
    frame.render_widget(
        Paragraph::new(host_line(view)).wrap(Wrap { trim: true }),
        host,
    );
    if preview.height >= 5 && preview.width >= 12 {
        preview_panel(frame, view, preview);
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!(
                "Last app: {}",
                fallback(&view.info.application, "unknown")
            )),
            Line::from(fallback(&view.info.capture, "Capture: unknown")),
        ]),
        application,
    );
    if let Some(message) = view.info.messages.back() {
        frame.render_widget(event_line(message), event);
    }
    frame.render_widget(footer_line(), footer);
}

fn header_line(view: &View) -> Line<'_> {
    Line::from(vec![
        Span::styled(" PRYSM ", Style::default().fg(BACKGROUND).bg(ACCENT).bold()),
        Span::styled("  TERMINAL  /  ", Style::default().fg(MUTED)),
        Span::styled(
            view.mode.label(),
            Style::default().fg(mode_color(view.mode)).bold(),
        ),
    ])
}

fn host_line(view: &View) -> Line<'_> {
    Line::from(vec![
        Span::styled("Host  ", Style::default().fg(MUTED)),
        Span::styled(
            fallback(&view.info.host.text, "starting"),
            Style::default()
                .fg(severity_color(view.info.host.severity))
                .bold(),
        ),
    ])
}

fn footer_line() -> Line<'static> {
    Line::from(vec![
        Span::styled(" Ctrl-C ", Style::default().fg(ACCENT).bold()),
        Span::styled("stop & release capture", Style::default().fg(MUTED)),
    ])
}

fn panel(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(title, Style::default().fg(TEXT)))
}

fn section<'a>(text: impl Into<std::borrow::Cow<'a, str>>) -> Line<'a> {
    Line::styled(text, Style::default().fg(MUTED))
}

fn fallback<'a>(value: &'a str, missing: &'a str) -> &'a str {
    if value.is_empty() { missing } else { value }
}

fn severity_color(severity: Severity) -> Color {
    match severity {
        Severity::Info => TEXT,
        Severity::Warning => Color::Rgb(242, 193, 102),
        Severity::Error => Color::Rgb(255, 132, 132),
    }
}

fn mode_color(mode: Mode) -> Color {
    match mode {
        Mode::Streaming => ACCENT,
        Mode::Held => severity_color(Severity::Warning),
        Mode::Idle | Mode::Configured | Mode::Off => MUTED,
    }
}

fn event_line(message: &Diagnostic) -> Line<'_> {
    let label = match message.severity {
        Severity::Info => "INFO ",
        Severity::Warning => "WARN ",
        Severity::Error => "ERROR",
    };
    Line::from(vec![
        Span::styled(
            label,
            Style::default().fg(severity_color(message.severity)).bold(),
        ),
        Span::raw("  "),
        Span::raw(&message.text),
    ])
}

fn preview_panel(frame: &mut Frame<'_>, view: &View, area: Rect) {
    let title = view.settings.as_ref().map_or_else(
        || " LED output · unconfigured ".to_owned(),
        |settings| format!(" LED output · {} LEDs ", settings.leds.iter().sum::<u32>()),
    );
    let block = panel(&title);
    let inside = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Edges(view), inside);
}

struct Edges<'a>(&'a View);
impl Widget for Edges<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.width < 4 || area.height < 3 {
            return;
        }
        let counts = self.0.settings.as_ref().map_or([0; 4], |s| s.leds);
        for i in 0..area.width {
            paint(
                buffer,
                (area.x + i, area.y),
                &self.0.colors.top,
                counts[0],
                i,
                area.width,
            );
            paint(
                buffer,
                (area.x + i, area.bottom() - 1),
                &self.0.colors.bottom,
                counts[2],
                i,
                area.width,
            );
        }
        for i in 0..area.height - 2 {
            paint(
                buffer,
                (area.x, area.y + 1 + i),
                &self.0.colors.left,
                counts[3],
                i,
                area.height - 2,
            );
            paint(
                buffer,
                (area.right() - 1, area.y + 1 + i),
                &self.0.colors.right,
                counts[1],
                i,
                area.height - 2,
            );
        }
        let center = Rect::new(area.x + 2, area.y + 1, area.width - 4, area.height - 2);
        let mut lines = vec![
            section(format!("TOP {}  →", counts[0])),
            Line::styled(
                self.0.mode.label(),
                Style::default()
                    .fg(mode_color(self.0.mode))
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(frame) = &self.0.frame {
            lines.extend([
                Line::from(format!(
                    "Frame {} · revision {}",
                    frame.stamp.sequence, frame.revision.0
                )),
                section(format!(
                    "Session {} · epoch {}",
                    frame.stamp.epoch.session.sequence, frame.stamp.epoch.generation
                )),
            ]);
        } else {
            lines.push(section(match self.0.mode {
                Mode::Held => "Held colors · awaiting frame",
                Mode::Off => "Output off",
                Mode::Streaming => "Waiting for admitted frame",
                Mode::Idle | Mode::Configured => "No admitted frame",
            }));
        }
        if center.height >= 6 {
            lines.extend([
                section(format!("LEFT {} ↓    RIGHT {} ↓", counts[3], counts[1])),
                section(format!("BOTTOM {}  →", counts[2])),
            ]);
        }
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .render(center, buffer);
    }
}

fn paint(
    buffer: &mut Buffer,
    position: (u16, u16),
    strip: &ColorStrip,
    count: u32,
    cell: u16,
    cells: u16,
) {
    if count == 0 {
        return;
    }
    let index = (u64::from(cell) * u64::from(count) / u64::from(cells)) as usize;
    let color = strip
        .color_at(index, count as usize)
        .expect("bounded LED position")
        .to_srgb();
    buffer[position]
        .set_symbol(" ")
        .set_style(Style::default().bg(Color::Rgb(color.r, color.g, color.b)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerminalSettings;
    use prysm_core::{ColorStrip, LinearColor};
    use ratatui::{Terminal, backend::TestBackend};
    use std::num::NonZeroUsize;

    fn contents(buffer: &Buffer) -> String {
        buffer.content.iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn edges_preserve_srgb_orientation_and_equal_led_width() {
        let mut view = View {
            settings: Some(TerminalSettings { leds: [2, 1, 1, 1] }),
            ..Default::default()
        };
        view.colors.top = ColorStrip::new(vec![
            LinearColor::new(1.0, 0.0, 0.0),
            LinearColor::new(0.0, 1.0, 0.0),
        ])
        .unwrap();
        view.colors.left = ColorStrip::fill(LinearColor::new(0.0, 0.0, 1.0), NonZeroUsize::MIN);
        let area = Rect::new(0, 0, 8, 5);
        let mut buffer = Buffer::empty(area);
        Edges(&view).render(area, &mut buffer);
        for x in 0..8 {
            assert_eq!(
                buffer[(x, 0)].bg,
                if x < 4 {
                    Color::Rgb(255, 0, 0)
                } else {
                    Color::Rgb(0, 255, 0)
                }
            );
        }
        assert_eq!(buffer[(0, 2)].bg, Color::Rgb(0, 0, 255));
    }

    #[test]
    fn resizing_preserves_host_status_and_prioritizes_latest_diagnostic() {
        let mut view = View::default();
        view.info.host = Diagnostic {
            severity: Severity::Error,
            text: "cleanup unconfirmed".into(),
        };
        view.info.application = "Session: failed; cleanup unconfirmed   Target: processed".into();
        view.info.capture = "Capture: waiting - no valid frame".into();
        view.info.revisions = "Revision configured / applied: 4 / 4".into();
        for index in 0..6 {
            view.info.messages.push_back(Diagnostic {
                severity: Severity::Error,
                text: format!("failure {index}"),
            });
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &view)).unwrap();
        let full = contents(terminal.backend().buffer());
        assert!(full.contains("cleanup unconfirmed"));
        assert!(full.contains("LAST APPLICATION SNAPSHOT"));
        assert!(full.contains("failure 5"));
        assert!(full.find("failure 5") < full.find("failure 4"));
        for (width, height) in [
            (40, 12),
            (60, 18),
            (72, 22),
            (20, 8),
            (1, 1),
            (0, 0),
            (100, 30),
        ] {
            terminal.backend_mut().resize(width, height);
            terminal.autoresize().unwrap();
            terminal.draw(|frame| render(frame, &view)).unwrap();
            if width >= 40 && height >= 12 {
                let text = contents(terminal.backend().buffer());
                assert!(text.contains("cleanup unconfirmed"), "{width}x{height}");
                assert!(text.contains("failure 5"), "{width}x{height}");
                assert!(text.contains("Ctrl-C"), "{width}x{height}");
                assert!(text.contains("Capture: waiting"), "{width}x{height}");
                assert!(text.contains("Session: failed"), "{width}x{height}");
            }
        }
    }
    #[test]
    fn newest_failure_wraps_before_older_diagnostics() {
        let mut view = View::default();
        view.info.messages.push_back(Diagnostic {
            severity: Severity::Info,
            text: "older event".into(),
        });
        view.info.messages.push_back(Diagnostic { severity: Severity::Error, text: "Operation 1: could not apply output configuration because required output capability is unavailable".into() });
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &view)).unwrap();
        let text = contents(terminal.backend().buffer());
        assert!(text.contains("capability is unavailable"));
        assert!(text.find("capability is unavailable") < text.find("older event"));
    }

    #[test]
    fn streaming_layout_keeps_frame_lineage_and_capture_visible() {
        use prysm_contract::{
            identity::{FrameStamp, InstanceId, SessionId, SettingsRevision, StreamEpoch},
            output::ColorFrame,
        };
        use std::num::NonZeroU64;
        let epoch = StreamEpoch {
            session: SessionId {
                instance: InstanceId(NonZeroU64::MIN),
                sequence: NonZeroU64::MIN,
            },
            generation: 2,
        };
        let colors = prysm_core::EdgeColors {
            top: ColorStrip::new(vec![
                LinearColor::new(0.6, 0.04, 0.02),
                LinearColor::new(0.03, 0.4, 0.8),
            ])
            .unwrap(),
            right: ColorStrip::fill(LinearColor::new(0.03, 0.4, 0.8), NonZeroUsize::MIN),
            bottom: ColorStrip::new(vec![
                LinearColor::new(0.4, 0.01, 0.5),
                LinearColor::new(0.03, 0.4, 0.8),
            ])
            .unwrap(),
            left: ColorStrip::fill(LinearColor::new(0.6, 0.04, 0.02), NonZeroUsize::MIN),
        };
        let mut view = View {
            mode: Mode::Streaming,
            settings: Some(TerminalSettings {
                leds: [30, 18, 30, 18],
            }),
            colors: colors.clone(),
            admitted: Some(epoch),
            frame: Some(ColorFrame {
                stamp: FrameStamp {
                    epoch,
                    sequence: 248,
                },
                revision: SettingsRevision(4),
                colors,
            }),
            ..Default::default()
        };
        view.info.host.text = "ready".into();
        view.info.application = "Session: running   Target: processed".into();
        view.info.capture = "Capture: available".into();
        view.info.revisions = "Revision configured / applied: 4 / 4".into();
        for (severity, text) in [
            (Severity::Info, "Capture available"),
            (Severity::Warning, "Example warning"),
            (Severity::Error, "Example error"),
        ] {
            view.info.messages.push_back(Diagnostic {
                severity,
                text: text.into(),
            });
        }
        for (width, height) in [(72, 22), (80, 24), (120, 32)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| render(frame, &view)).unwrap();
            let buffer = terminal.backend().buffer();
            let text = contents(buffer);
            assert!(text.contains("Frame 248"), "{width}x{height}");
            assert!(text.contains("Capture: available"), "{width}x{height}");
            assert!(text.contains("4 / 4"), "{width}x{height}");
            assert!(text.contains("WARN "));
            assert!(text.contains("ERROR"));
        }
    }
}
