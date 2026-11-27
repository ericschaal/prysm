//! Pure presentation: snapshot, preview and UI state in, cells out.
use crate::input::{Notice, Severity, Ui, clean};
use crate::video;
use prysm_core::{ColorStrip, EdgeColors};
use prysm_ports::{AdapterSettings, Error, Picture, SessionState, Snapshot, WaitingFor, chain};
use prysm_runtime::Preview;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
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
            .filter(|_| matches!(self.snapshot.session, SessionState::ScreenSync))
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
    let mut lines: Vec<_> = diagnostics(view).take(rows).collect();
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
    let mut summary = format!(
        "Target {} · Session {}",
        snapshot.target.as_str(),
        session_label(snapshot)
    );
    if let Some(input) = input_label(snapshot) {
        summary.push_str(" · Input: ");
        summary.push_str(&input);
    }
    if let Some(issue) = snapshot.device.issue {
        summary.push_str(" · Lights: ");
        summary.push_str(&issue.to_string());
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(summary),
            Line::from(format!(
                "{} → {} · rev {}",
                adapter_label(snapshot.settings.capture.as_ref()),
                adapter_label(snapshot.settings.output.as_ref()),
                snapshot.revision
            )),
        ]),
        status,
    );
    if let Some(line) = diagnostics(view).next() {
        frame.render_widget(line, event);
    }
    footer_line(frame, view.ui, footer);
}

fn header_line(snapshot: &Snapshot) -> Line<'static> {
    Line::from(vec![
        Span::styled(" PRYSM ", Style::default().fg(BACKGROUND).bg(ACCENT).bold()),
        Span::styled("  TERMINAL  /  ", Style::default().fg(MUTED)),
        styled_session(snapshot),
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
        field("Target", snapshot.target.as_str().to_owned()),
        Line::from(vec![
            Span::styled("Session   ", Style::default().fg(MUTED)),
            styled_session(snapshot),
        ]),
    ];
    if let Some(input) = input_label(snapshot) {
        lines.push(field("Input", input));
    }
    if let Some(issue) = snapshot.device.issue {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<10}", "Lights"), Style::default().fg(MUTED)),
            Span::styled(issue.to_string(), Style::default().fg(WARNING)),
        ]));
    }
    lines.extend([
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
    ]);
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

/// Lighting failure, surface failures, then notices; newest first. Each
/// failure ends with the person's next step.
fn diagnostics(view: &View<'_>) -> impl Iterator<Item = Line<'static>> {
    let snapshot = view.snapshot;
    let lighting = match &snapshot.session {
        SessionState::Faulted(phase) => Some(failed("Lighting", &Error::Faulted(*phase))),
        session => session.failure().map(|error| {
            if snapshot.control.waiting.is_some() {
                Notice::info(format!("Lighting waiting: {}", chain(error)))
            } else {
                failed("Lighting", error)
            }
        }),
    };
    let surfaces = snapshot
        .surface_failures
        .iter()
        .rev()
        .map(|error| failed("Surface", error));
    lighting
        .into_iter()
        .chain(surfaces)
        .chain(view.ui.notices().cloned())
        .map(event_line)
}

fn failed(what: &str, error: &Error) -> Notice {
    let step = error.escalation().next_step();
    Notice::error(format!("{what}: {} ({step})", chain(error)))
}

fn event_line(notice: Notice) -> Line<'static> {
    let (label, color) = match notice.severity {
        Severity::Info => ("INFO ", TEXT),
        Severity::Error => ("ERROR", ERROR),
    };
    Line::from(vec![
        Span::styled(label, Style::default().fg(color).bold()),
        Span::raw("  "),
        Span::raw(notice.text),
    ])
}

fn panel(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(title, Style::default().fg(TEXT)))
}

fn session_label(snapshot: &Snapshot) -> String {
    match snapshot.control.waiting {
        Some(WaitingFor::Capture) => "waiting for capture".into(),
        Some(WaitingFor::Output) => "waiting for lights".into(),
        None => snapshot.session.to_string(),
    }
}

fn styled_session(snapshot: &Snapshot) -> Span<'static> {
    let color = if snapshot.control.waiting.is_some() {
        WARNING
    } else {
        match &snapshot.session {
            SessionState::ScreenSync => ACCENT,
            SessionState::Effects { .. } | SessionState::Unanswered { .. } => WARNING,
            SessionState::Faulted(_) => ERROR,
            SessionState::Stopped { .. } => MUTED,
        }
    };
    Span::styled(session_label(snapshot), Style::default().fg(color).bold())
}

/// Frame rate while frames arrive, why not while the running source gives
/// none, nothing without a session.
pub(crate) fn input_label(snapshot: &Snapshot) -> Option<String> {
    snapshot.input.map(|input| match input.picture {
        Picture::Arriving => format!("{:.1} fps", input.fps),
        Picture::Quiet => "waiting for frames".to_owned(),
        Picture::Refused => "capture not allowed".to_owned(),
    })
}

fn adapter_label(settings: Option<&AdapterSettings>) -> &str {
    settings.map_or("not configured", |settings| settings.kind.as_str())
}

/// The LED perimeter with the video inside it.
fn leds_panel(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let shown = view.shown();
    let title = match shown {
        Some(preview) => format!(" LEDs · {} samples ", samples(&preview.colors)),
        None => " LEDs ".to_owned(),
    };
    let block = panel(&title);
    let inside = block.inner(area);
    frame.render_widget(block, area);
    let Some(center) = center(inside) else {
        return;
    };
    let placeholder = match shown {
        Some(preview) => {
            paint_edges(frame.buffer_mut(), inside, &preview.colors);
            match video::draw(&preview.frame, center, frame.buffer_mut()) {
                Ok(()) => return,
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
    match &snapshot.session {
        SessionState::ScreenSync => "Waiting for the first frame",
        SessionState::Effects { .. } => "The lights show their own effects",
        SessionState::Unanswered { .. } => "The lights do not answer; Prysm keeps trying",
        SessionState::Faulted(_) => "Session did not finish; only shutdown is accepted",
        SessionState::Stopped { failure: Some(_) } => "Lighting stopped after a failure",
        SessionState::Stopped { failure: None } => "Lighting off · press s to start",
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
fn paint_edges(buffer: &mut Buffer, area: Rect, colors: &EdgeColors) {
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

fn paint(buffer: &mut Buffer, position: (u16, u16), strip: &ColorStrip, cell: u16, cells: u16) {
    let index = usize::from(cell) * strip.len() / usize::from(cells);
    let Some(color) = strip.samples().get(index) else {
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
    use prysm_core::{Frame, LinearColor, PixelFormat, Viewport};
    use prysm_ports::{Error, LightingTarget, Revision};
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
            target: LightingTarget::ScreenSync,
            session: SessionState::ScreenSync,
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
            viewport: Viewport::full_frame(32, 16),
            sequence: 1,
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
        paint_edges(&mut buffer, area, &colors);
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

        for (width, height) in [(0, 5), (8, 0), (8, 1), (8, 2), (4, 5)] {
            let small = Rect::new(0, 0, width, height);
            let mut buffer = Buffer::empty(small);
            paint_edges(&mut buffer, small, &colors);
            assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
        }
    }

    #[test]
    fn absent_preview_leaves_the_led_perimeter_unpainted() {
        let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
        let view = View {
            snapshot: &running(),
            preview: None,
            ui: &Ui::default(),
        };
        terminal
            .draw(|frame| leds_panel(frame, &view, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
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
        let first = buffer
            .content
            .iter()
            .position(|cell| cell.symbol() == "▀")
            .unwrap();
        let width = usize::from(buffer.area.width);
        assert_eq!(buffer.content[first - width].bg, Color::Rgb(255, 0, 0));
        assert!(text.contains("32×16 RGB24"), "{text}");
        assert!(text.contains("1 · 3 · 2 · 3"));
        assert!(text.contains("screen_sync"));
        assert!(text.contains("device"));
    }

    #[test]
    fn stopped_sessions_show_no_stale_preview() {
        let snapshot = Snapshot {
            session: SessionState::default(),
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
        assert!(text.contains("press s to start"));
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
            session: SessionState::Effects {
                failure: Some(Error::adapter(
                    "capture failed",
                    std::io::Error::other("device unplugged"),
                )),
            },
            surface_failures: vec![Error::other("portal could not bind")],
            ..Snapshot::default()
        };
        let mut ui = Ui::default();
        ui.notify(Notice::info("older event"));
        ui.notify(Notice::error("Target screen_sync: newest event"));
        let view = View {
            snapshot: &snapshot,
            preview: None,
            ui: &ui,
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let text = draw(&mut terminal, &view);
        assert!(
            text.contains("capture failed: device unplugged (try again)"),
            "{text}"
        );
        assert!(text.contains("portal could not bind (try again)"));
        assert!(text.contains("Session   effects"), "{text}");
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
                assert!(text.contains("effects"), "{width}x{height}");
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
        assert!(
            text.contains("Target screen_sync · Session screen_sync"),
            "{text}"
        );
        assert!(text.contains("device → null · rev 4"));
    }

    #[test]
    fn preview_errors_replace_the_video_without_failing() {
        let snapshot = running();
        let preview = Preview {
            frame: Frame::dummy(0, 2).unwrap(),
            colors: EdgeColors::default(),
            viewport: Viewport::full_frame(0, 2),
            sequence: 1,
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

    #[test]
    fn status_shows_frame_rate_or_waiting_while_running() {
        let mut snapshot = running();
        snapshot.input = Some(prysm_ports::Input {
            fps: 29.94,
            picture: Picture::Arriving,
            ..prysm_ports::Input::default()
        });
        let ui = Ui::default();
        let view = View {
            snapshot: &snapshot,
            preview: None,
            ui: &ui,
        };
        let text = draw(
            &mut Terminal::new(TestBackend::new(100, 30)).unwrap(),
            &view,
        );
        assert!(text.contains("29.9 fps"), "{text}");
        let text = draw(&mut Terminal::new(TestBackend::new(70, 18)).unwrap(), &view);
        assert!(text.contains("Input: 29.9 fps"), "{text}");

        snapshot.input = Some(prysm_ports::Input::default());
        let view = View {
            snapshot: &snapshot,
            preview: None,
            ui: &ui,
        };
        let text = draw(
            &mut Terminal::new(TestBackend::new(100, 30)).unwrap(),
            &view,
        );
        assert!(text.contains("waiting for frames"), "{text}");

        snapshot.input = Some(prysm_ports::Input {
            picture: Picture::Refused,
            ..prysm_ports::Input::default()
        });
        let view = View {
            snapshot: &snapshot,
            preview: None,
            ui: &ui,
        };
        let text = draw(
            &mut Terminal::new(TestBackend::new(100, 30)).unwrap(),
            &view,
        );
        assert!(text.contains("capture not allowed"), "{text}");

        snapshot.input = None;
        let view = View {
            snapshot: &snapshot,
            preview: None,
            ui: &ui,
        };
        let text = draw(
            &mut Terminal::new(TestBackend::new(100, 30)).unwrap(),
            &view,
        );
        assert!(!text.contains("Input"), "{text}");
    }
    #[test]
    fn a_faulted_session_says_what_did_not_finish_at_every_size() {
        let snapshot = Snapshot {
            session: SessionState::Faulted(prysm_ports::SessionPhase::ClosingSource),
            ..Snapshot::default()
        };
        let ui = Ui::default();
        let view = View {
            snapshot: &snapshot,
            preview: None,
            ui: &ui,
        };
        for (width, height) in [(100, 30), (70, 18)] {
            let text = draw(
                &mut Terminal::new(TestBackend::new(width, height)).unwrap(),
                &view,
            );
            assert!(
                text.contains("faulted while closing the source"),
                "{width}x{height}: {text}"
            );
        }
        let text = draw(
            &mut Terminal::new(TestBackend::new(100, 30)).unwrap(),
            &view,
        );
        assert!(
            text.contains("Session did not finish; only shutdown is accepted"),
            "{text}"
        );
        assert!(text.contains("(restart Prysm)"), "{text}");
    }

    #[test]
    fn status_says_why_the_lights_do_not_show_prysm() {
        let mut snapshot = running();
        snapshot.device.issue = Some(prysm_ports::OutputIssue::Overridden);
        let ui = Ui::default();
        let view = View {
            snapshot: &snapshot,
            preview: None,
            ui: &ui,
        };
        let text = draw(
            &mut Terminal::new(TestBackend::new(100, 30)).unwrap(),
            &view,
        );
        assert!(text.contains("Lights"), "{text}");
        assert!(text.contains("live override"), "{text}");
        let text = draw(&mut Terminal::new(TestBackend::new(70, 18)).unwrap(), &view);
        assert!(text.contains("Lights: live override is on"), "{text}");
    }
}
