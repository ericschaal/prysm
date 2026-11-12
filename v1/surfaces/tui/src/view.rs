//! Pure presentation: snapshot, preview and UI state in, cells out.
use crate::command::{describe, target_label};
use crate::input::{Notice, Severity, Ui, clean};
use crate::video::{self, HalfBlocks};
use prysm_core::{ColorStrip, EdgeColors};
use prysm_ports::{AdapterSettings, SessionState, Snapshot};
use prysm_runtime::Preview;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Widget, Wrap},
};

const BACKGROUND: Color = Color::Rgb(16, 20, 28);
const TEXT: Color = Color::Rgb(225, 231, 239);
const MUTED: Color = Color::Rgb(150, 164, 183);
const BORDER: Color = Color::Rgb(64, 80, 101);
const ACCENT: Color = Color::Rgb(102, 213, 219);
const WARNING: Color = Color::Rgb(242, 193, 102);
const ERROR: Color = Color::Rgb(255, 132, 132);
/// Terminal cells are about twice as tall as wide, so side strips are two cells.
const SIDE_THICKNESS: u16 = 2;
/// Below this size the layout keeps only the essentials.
const FULL_WIDTH: u16 = 72;
const FULL_HEIGHT: u16 = 22;

/// Everything one redraw shows.
pub(crate) struct View<'a> {
    pub snapshot: &'a Snapshot,
    /// The latest preview; shown only while a session runs.
    pub preview: Option<&'a Preview>,
    pub ui: &'a Ui,
}

impl View<'_> {
    /// What the LEDs show now: nothing unless a session is running.
    fn shown(&self) -> Option<&Preview> {
        self.preview
            .filter(|_| self.snapshot.session == SessionState::Running)
    }
}

pub(crate) fn render(frame: &mut Frame<'_>, view: &View<'_>) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKGROUND).fg(TEXT)),
        area,
    );
    if area.width < FULL_WIDTH || area.height < FULL_HEIGHT {
        compact(frame, view);
        return;
    }
    let [header, content, events, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(if area.height >= 28 { 8 } else { 6 }),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(header_line(view.snapshot), header);
    let [leds, status] = Layout::horizontal([Constraint::Min(24), Constraint::Length(34)])
        .spacing(1)
        .areas(content);
    leds_panel(frame, view, leds);
    frame.render_widget(
        Paragraph::new(status_lines(view))
            .wrap(Wrap { trim: true })
            .block(panel(" Status ")),
        status,
    );
    let block = panel(" Diagnostics · newest first ");
    let rows = usize::from(block.inner(events).height);
    let mut lines: Vec<_> = diagnostics(view).into_iter().take(rows).collect();
    if lines.is_empty() {
        lines.push(Line::styled("No events yet", Style::default().fg(MUTED)));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(block),
        events,
    );
    footer_line(frame, view.ui, footer);
}

fn compact(frame: &mut Frame<'_>, view: &View<'_>) {
    let [header, leds, status, event, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    frame.render_widget(header_line(view.snapshot), header);
    if leds.height >= 5 && leds.width >= 12 {
        leds_panel(frame, view, leds);
    }
    let snapshot = view.snapshot;
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!(
                "Target: {} · Session: {}",
                target_label(snapshot.target),
                session_label(snapshot.session)
            )),
            Line::from(format!(
                "{} → {} · rev {}",
                adapter_label(snapshot.settings.capture.as_ref()),
                adapter_label(snapshot.settings.output.as_ref()),
                snapshot.revision
            )),
        ]),
        status,
    );
    if let Some(line) = diagnostics(view).into_iter().next() {
        frame.render_widget(line, event);
    }
    footer_line(frame, view.ui, footer);
}

fn header_line(snapshot: &Snapshot) -> Line<'static> {
    Line::from(vec![
        Span::styled(" PRYSM ", Style::default().fg(BACKGROUND).bg(ACCENT).bold()),
        Span::styled("  TERMINAL  /  ", Style::default().fg(MUTED)),
        Span::styled(
            session_label(snapshot.session),
            Style::default().fg(session_color(snapshot.session)).bold(),
        ),
    ])
}

fn footer_line(frame: &mut Frame<'_>, ui: &Ui, area: Rect) {
    let text = ui.footer(usize::from(area.width));
    frame.render_widget(Line::styled(text, Style::default().fg(MUTED)), area);
}

/// The right-hand status panel.
pub(crate) fn status_lines(view: &View<'_>) -> Vec<Line<'static>> {
    let snapshot = view.snapshot;
    let mut lines = vec![
        field("Target", target_label(snapshot.target).to_owned()),
        Line::from(vec![
            Span::styled("Session   ", Style::default().fg(MUTED)),
            Span::styled(
                session_label(snapshot.session),
                Style::default().fg(session_color(snapshot.session)).bold(),
            ),
        ]),
        field("Revision", snapshot.revision.to_string()),
        Line::default(),
        field(
            "Capture",
            adapter_label(snapshot.settings.capture.as_ref()).into(),
        ),
        field(
            "Output",
            adapter_label(snapshot.settings.output.as_ref()).into(),
        ),
    ];
    if let Some(preview) = view.shown() {
        let frame = &preview.frame;
        let colors = &preview.colors;
        lines.extend([
            Line::default(),
            field(
                "Frame",
                format!("{}×{} {}", frame.width(), frame.height(), frame.format()),
            ),
            field(
                "LEDs",
                format!(
                    "{} · {} · {} · {}",
                    colors.top.len(),
                    colors.right.len(),
                    colors.bottom.len(),
                    colors.left.len()
                ),
            ),
            Line::styled(
                "          top·right·bottom·left",
                Style::default().fg(MUTED),
            ),
        ]);
    }
    lines
}

fn field(name: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{name:<10}"), Style::default().fg(MUTED)),
        Span::raw(clean(&value)),
    ])
}

/// Lighting failure, surface failures, then notices; newest first.
pub(crate) fn diagnostics(view: &View<'_>) -> Vec<Line<'static>> {
    let snapshot = view.snapshot;
    let failures = snapshot
        .failure
        .iter()
        .map(|error| Notice::error(format!("Lighting: {}", describe(error))));
    let surfaces = snapshot
        .surface_failures
        .iter()
        .rev()
        .map(|error| Notice::error(format!("Surface: {}", describe(error))));
    failures
        .chain(surfaces)
        .chain(view.ui.notices().cloned())
        .map(|notice| event_line(&notice))
        .collect()
}

fn event_line(notice: &Notice) -> Line<'static> {
    let (label, color) = match notice.severity {
        Severity::Info => ("INFO ", TEXT),
        Severity::Error => ("ERROR", ERROR),
    };
    Line::from(vec![
        Span::styled(label, Style::default().fg(color).bold()),
        Span::raw("  "),
        Span::raw(notice.text.clone()),
    ])
}

fn panel(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(title, Style::default().fg(TEXT)))
}

pub(crate) fn session_label(state: SessionState) -> String {
    match state {
        SessionState::Stopped => "stopped".into(),
        SessionState::Running => "running".into(),
        SessionState::Native => "native".into(),
        SessionState::Faulted(phase) => format!("faulted while {phase}"),
    }
}

fn session_color(state: SessionState) -> Color {
    match state {
        SessionState::Running => ACCENT,
        SessionState::Native => WARNING,
        SessionState::Faulted(_) => ERROR,
        SessionState::Stopped => MUTED,
    }
}

fn adapter_label(settings: Option<&AdapterSettings>) -> &str {
    settings.map_or("not configured", |settings| settings.kind.as_str())
}

/// The LED perimeter with the video inside it.
fn leds_panel(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let title = match view.shown() {
        Some(preview) => format!(" LEDs · {} samples ", samples(&preview.colors)),
        None => " LEDs ".to_owned(),
    };
    let block = panel(&title);
    let inside = block.inner(area);
    frame.render_widget(block, area);
    let shown = view.shown();
    frame.render_widget(Edges(shown.map(|preview| &preview.colors)), inside);
    let Some(center) = center(inside) else {
        return;
    };
    let placeholder = match shown {
        Some(preview) => {
            match video::sample(
                &preview.frame,
                u32::from(center.width),
                u32::from(center.height) * 2,
            ) {
                Ok(image) => {
                    frame.render_widget(HalfBlocks(&image), center);
                    return;
                }
                Err(reason) => format!("Preview unavailable: {reason}"),
            }
        }
        None => idle_text(view.snapshot).to_owned(),
    };
    frame.render_widget(
        Paragraph::new(placeholder)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(MUTED)),
        vertical_middle(center),
    );
}

fn samples(colors: &EdgeColors) -> usize {
    colors.top.len() + colors.right.len() + colors.bottom.len() + colors.left.len()
}

fn idle_text(snapshot: &Snapshot) -> &'static str {
    match snapshot.session {
        SessionState::Running => "Waiting for the first frame",
        SessionState::Native => "The output shows its native effect",
        SessionState::Faulted(_) => "Session did not stop; only shutdown is accepted",
        SessionState::Stopped if snapshot.failure.is_some() => "Lighting stopped after a failure",
        SessionState::Stopped => "Lighting off · press p to start",
    }
}

/// The area inside the LED strips.
fn center(inside: Rect) -> Option<Rect> {
    (inside.width > 2 * SIDE_THICKNESS && inside.height >= 3).then(|| {
        Rect::new(
            inside.x + SIDE_THICKNESS,
            inside.y + 1,
            inside.width - 2 * SIDE_THICKNESS,
            inside.height - 2,
        )
    })
}

fn vertical_middle(area: Rect) -> Rect {
    let offset = area.height.saturating_sub(1) / 2;
    Rect::new(area.x, area.y + offset, area.width, area.height - offset)
}

/// The four edge strips, in screen orientation: top and bottom left to right,
/// left and right top to bottom. Each cell shows the sample at its position.
pub(crate) struct Edges<'a>(pub Option<&'a EdgeColors>);

impl Widget for Edges<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let Some(colors) = self.0 else {
            return;
        };
        if center(area).is_none() {
            return;
        }
        for i in 0..area.width {
            paint(buffer, (area.x + i, area.y), &colors.top, i, area.width);
            paint(
                buffer,
                (area.x + i, area.bottom() - 1),
                &colors.bottom,
                i,
                area.width,
            );
        }
        let rows = area.height - 2;
        for i in 0..rows {
            for offset in 0..SIDE_THICKNESS {
                let y = area.y + 1 + i;
                paint(buffer, (area.x + offset, y), &colors.left, i, rows);
                paint(
                    buffer,
                    (area.right() - 1 - offset, y),
                    &colors.right,
                    i,
                    rows,
                );
            }
        }
    }
}

fn paint(buffer: &mut Buffer, position: (u16, u16), strip: &ColorStrip, cell: u16, cells: u16) {
    let count = strip.len();
    if count == 0 {
        return;
    }
    let index = usize::from(cell) * count / usize::from(cells);
    let Some(color) = strip.color_at(index, count) else {
        return;
    };
    let color = color.to_srgb();
    buffer[position]
        .set_symbol(" ")
        .set_style(Style::default().bg(Color::Rgb(color.r, color.g, color.b)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Notice;
    use prysm_core::{Frame, LinearColor, PixelFormat};
    use prysm_ports::{Error, LightingTarget, Revision, SessionPhase};
    use ratatui::{Terminal, backend::TestBackend};
    use std::num::NonZeroUsize;

    fn contents(buffer: &Buffer) -> String {
        buffer.content.iter().map(|cell| cell.symbol()).collect()
    }

    fn draw(terminal: &mut Terminal<TestBackend>, view: &View<'_>) -> String {
        terminal.draw(|frame| render(frame, view)).unwrap();
        contents(terminal.backend().buffer())
    }

    fn strip(colors: &[(f32, f32, f32)]) -> ColorStrip {
        ColorStrip::new(
            colors
                .iter()
                .map(|&(r, g, b)| LinearColor::new(r, g, b))
                .collect(),
        )
        .unwrap()
    }

    fn running() -> Snapshot {
        Snapshot {
            target: LightingTarget::Processed,
            session: SessionState::Running,
            revision: Revision(4),
            settings: serde_json::from_str(
                r#"{ "capture": { "kind": "device" }, "output": { "kind": "null" } }"#,
            )
            .unwrap(),
            ..Snapshot::default()
        }
    }

    fn green_preview() -> Preview {
        Preview {
            frame: Frame::new([0, 255, 0].repeat(32 * 16), 32, 16, PixelFormat::RGB24).unwrap(),
            colors: EdgeColors {
                top: strip(&[(1.0, 0.0, 0.0)]),
                right: strip(&[(0.0, 0.0, 1.0); 3]),
                bottom: strip(&[(0.0, 0.0, 1.0); 2]),
                left: strip(&[(0.0, 0.0, 1.0); 3]),
            },
        }
    }

    #[test]
    fn edges_preserve_srgb_orientation_and_equal_led_width() {
        let colors = EdgeColors {
            top: strip(&[(1.0, 0.0, 0.0), (0.0, 1.0, 0.0)]),
            left: ColorStrip::fill(LinearColor::new(0.0, 0.0, 1.0), NonZeroUsize::MIN),
            ..EdgeColors::default()
        };
        let area = Rect::new(0, 0, 8, 5);
        let mut buffer = Buffer::empty(area);
        Edges(Some(&colors)).render(area, &mut buffer);
        for x in 0..8 {
            let expected = if x < 4 {
                Color::Rgb(255, 0, 0)
            } else {
                Color::Rgb(0, 255, 0)
            };
            assert_eq!(buffer[(x, 0)].bg, expected);
        }
        assert_eq!(buffer[(0, 2)].bg, Color::Rgb(0, 0, 255));
        assert_eq!(buffer[(1, 2)].bg, Color::Rgb(0, 0, 255));
        assert_eq!(buffer[(6, 2)].bg, buffer[(7, 2)].bg);
        assert_eq!(buffer[(2, 2)].bg, Color::Reset);

        let mut dark = Buffer::empty(area);
        Edges(None).render(area, &mut dark);
        assert!(dark.content.iter().all(|cell| cell.bg == Color::Reset));
    }

    #[test]
    fn video_fills_the_inside_of_the_led_perimeter() {
        let snapshot = running();
        let preview = green_preview();
        let ui = Ui::default();
        let view = View {
            snapshot: &snapshot,
            preview: Some(&preview),
            ui: &ui,
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let text = draw(&mut terminal, &view);
        let buffer = terminal.backend().buffer();
        let green = Color::Rgb(0, 255, 0);
        let video: Vec<_> = buffer
            .content
            .iter()
            .filter(|cell| cell.symbol() == "▀")
            .collect();
        assert!(video.len() > 100);
        assert!(
            video
                .iter()
                .all(|cell| cell.fg == green && cell.bg == green)
        );
        // The top strip is red above the first video row.
        let first = buffer
            .content
            .iter()
            .position(|cell| cell.symbol() == "▀")
            .unwrap();
        let width = usize::from(buffer.area.width);
        assert_eq!(buffer.content[first - width].bg, Color::Rgb(255, 0, 0));
        assert!(text.contains("32×16 RGB24"), "{text}");
        assert!(text.contains("1 · 3 · 2 · 3"));
        assert!(text.contains("running"));
        assert!(text.contains("device"));
    }

    #[test]
    fn stopped_sessions_show_no_stale_preview() {
        let snapshot = Snapshot {
            session: SessionState::Stopped,
            ..running()
        };
        let preview = green_preview();
        let ui = Ui::default();
        let view = View {
            snapshot: &snapshot,
            preview: Some(&preview),
            ui: &ui,
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let text = draw(&mut terminal, &view);
        assert!(!text.contains('▀'));
        assert!(text.contains("press p to start"));
        let red = Color::Rgb(255, 0, 0);
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|c| c.bg != red)
        );
    }

    #[test]
    fn failures_and_notices_are_shown_newest_first_at_every_size() {
        let snapshot = Snapshot {
            session: SessionState::Faulted(SessionPhase::ClosingSource),
            failure: Some(Error::adapter(
                "capture failed",
                std::io::Error::other("device unplugged"),
            )),
            surface_failures: vec![Error::other("portal could not bind")],
            ..Snapshot::default()
        };
        let mut ui = Ui::default();
        ui.notify(Notice::info("older event"));
        ui.notify(Notice::error("Target processed: newest event"));
        let view = View {
            snapshot: &snapshot,
            preview: None,
            ui: &ui,
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let text = draw(&mut terminal, &view);
        assert!(text.contains("capture failed: device unplugged"), "{text}");
        assert!(text.contains("portal could not bind"));
        assert!(text.contains("faulted while closing the source"));
        assert!(text.find("newest event") < text.find("older event"));
        assert!(text.contains("q quit"));
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
            let text = draw(&mut terminal, &view);
            if width >= 40 && height >= 12 {
                assert!(text.contains("faulted"), "{width}x{height}");
                assert!(text.contains("capture failed"), "{width}x{height}");
            }
        }
    }

    #[test]
    fn compact_layout_keeps_video_and_status() {
        let snapshot = running();
        let preview = green_preview();
        let ui = Ui::default();
        let view = View {
            snapshot: &snapshot,
            preview: Some(&preview),
            ui: &ui,
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        let text = draw(&mut terminal, &view);
        assert!(text.contains('▀'));
        assert!(text.contains("Target: processed · Session: running"));
        assert!(text.contains("device → null · rev 4"));
    }

    #[test]
    fn preview_errors_replace_the_video_without_failing() {
        let snapshot = running();
        let preview = Preview {
            frame: Frame::new(vec![1, 2, 3], 4, 2, PixelFormat::MJPEG).unwrap(),
            colors: EdgeColors::default(),
        };
        let ui = Ui::default();
        let view = View {
            snapshot: &snapshot,
            preview: Some(&preview),
            ui: &ui,
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let text = draw(&mut terminal, &view);
        assert!(text.contains("Preview unavailable"), "{text}");
    }
}
