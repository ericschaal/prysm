//! Key presses become actions; the command bar and notices are local UI state.
use crate::command::Command;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use prysm_ports::LightingTarget;
use std::collections::VecDeque;
use std::path::PathBuf;

const MAX_DRAFT: usize = 4096;
const MAX_NOTICES: usize = 6;
const MAX_NOTICE_CHARS: usize = 240;

/// What a key press asks of the surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    Run(Command),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    Info,
    Error,
}

/// A command result or input error, shown under diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    pub severity: Severity,
    pub text: String,
}

impl Notice {
    pub fn info(text: impl AsRef<str>) -> Self {
        Self::new(Severity::Info, text.as_ref())
    }

    pub fn error(text: impl AsRef<str>) -> Self {
        Self::new(Severity::Error, text.as_ref())
    }

    fn new(severity: Severity, text: &str) -> Self {
        Self {
            severity,
            text: clean(text),
        }
    }
}

/// Terminal-local state: the command bar draft and recent notices.
#[derive(Debug, Default)]
pub(crate) struct Ui {
    draft: Option<String>,
    notices: VecDeque<Notice>,
}

impl Ui {
    pub fn key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if control && matches!(key.code, KeyCode::Char('c' | 'd')) {
            return Some(Action::Quit);
        }
        if control || key.modifiers.contains(KeyModifiers::ALT) {
            return None;
        }
        let Some(draft) = &mut self.draft else {
            return self.shortcut(key.code);
        };
        match key.code {
            KeyCode::Esc => self.draft = None,
            KeyCode::Backspace => {
                draft.pop();
            }
            KeyCode::Char(c) if !c.is_control() && draft.len() + c.len_utf8() <= MAX_DRAFT => {
                draft.push(c);
            }
            KeyCode::Enter => {
                let line = self.draft.take().unwrap_or_default();
                return match parse(&line) {
                    Ok(action) => action,
                    Err(error) => {
                        self.notify(Notice::error(error));
                        None
                    }
                };
            }
            _ => {}
        }
        None
    }

    fn shortcut(&mut self, code: KeyCode) -> Option<Action> {
        let target = |target| Some(Action::Run(Command::SetTarget(target)));
        match code {
            KeyCode::Char('p') => target(LightingTarget::Processed),
            KeyCode::Char('n') => target(LightingTarget::DeviceNative),
            KeyCode::Char('o') => target(LightingTarget::Off),
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
            KeyCode::Char(':') => {
                self.draft = Some(String::new());
                None
            }
            // Scroll-generated arrows and anything else are ignored.
            _ => None,
        }
    }

    pub fn notify(&mut self, notice: Notice) {
        if self.notices.len() == MAX_NOTICES {
            self.notices.pop_front();
        }
        self.notices.push_back(notice);
    }

    /// Newest first.
    pub fn notices(&self) -> impl Iterator<Item = &Notice> {
        self.notices.iter().rev()
    }

    /// The footer: the command being typed, or the key help. Keeps the end
    /// visible on narrow terminals without splitting characters.
    pub fn footer(&self, width: usize) -> String {
        let text = match &self.draft {
            Some(draft) => format!(" Enter run · Esc cancel  :{draft}▏"),
            None => HELP.to_owned(),
        };
        let skip = match &self.draft {
            Some(_) => text.chars().count().saturating_sub(width),
            None => 0,
        };
        text.chars().skip(skip).take(width).collect()
    }
}

pub(crate) const HELP: &str =
    " p processed · n native · o off · : command (on|off|native|load <path>) · q quit";

fn parse(line: &str) -> Result<Option<Action>, String> {
    let line = line.trim();
    let run = |target| Ok(Some(Action::Run(Command::SetTarget(target))));
    match line {
        "" => Ok(None),
        "on" | "processed" => run(LightingTarget::Processed),
        "off" => run(LightingTarget::Off),
        "native" => run(LightingTarget::DeviceNative),
        "q" | "quit" => Ok(Some(Action::Quit)),
        _ => match line.strip_prefix("load ").map(str::trim) {
            Some(path) if !path.is_empty() => {
                Ok(Some(Action::Run(Command::Load(PathBuf::from(path)))))
            }
            _ => Err(format!(
                "Unknown command `{line}`; use on, off, native or load <path>"
            )),
        },
    }
}

/// Bounded and free of control characters, so text cannot corrupt the screen.
pub(crate) fn clean(text: &str) -> String {
    text.chars()
        .take(MAX_NOTICE_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(ui: &mut Ui, text: &str) -> Vec<Action> {
        text.chars()
            .filter_map(|c| ui.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)))
            .collect()
    }

    fn set(target: LightingTarget) -> Action {
        Action::Run(Command::SetTarget(target))
    }

    #[test]
    fn shortcuts_set_targets_and_quit() {
        let mut ui = Ui::default();
        assert_eq!(
            press(&mut ui, "pnox"),
            [
                set(LightingTarget::Processed),
                set(LightingTarget::DeviceNative),
                set(LightingTarget::Off)
            ]
        );
        assert_eq!(ui.key(KeyCode::Up.into()), None);
        assert_eq!(press(&mut ui, "q"), [Action::Quit]);
        assert_eq!(ui.key(KeyCode::Esc.into()), Some(Action::Quit));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(ui.key(ctrl_c), Some(Action::Quit));
        let mut release = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(ui.key(release), None);
    }

    #[test]
    fn command_bar_edits_and_only_enter_submits() {
        let mut ui = Ui::default();
        // Shortcuts are plain text while typing.
        assert!(press(&mut ui, ":load /tmp/café x.jsonq").is_empty());
        ui.key(KeyCode::Backspace.into());
        assert!(ui.footer(100).ends_with(":load /tmp/café x.json▏"));
        assert_eq!(
            ui.key(KeyCode::Enter.into()),
            Some(Action::Run(Command::Load("/tmp/café x.json".into())))
        );
        press(&mut ui, ":off");
        ui.key(KeyCode::Esc.into());
        assert_eq!(ui.key(KeyCode::Enter.into()), None);
        assert_eq!(ui.footer(200), HELP);
        press(&mut ui, ":native");
        assert_eq!(
            ui.key(KeyCode::Enter.into()),
            Some(set(LightingTarget::DeviceNative))
        );
    }

    #[test]
    fn unknown_commands_become_error_notices() {
        let mut ui = Ui::default();
        press(&mut ui, ":dance");
        assert_eq!(ui.key(KeyCode::Enter.into()), None);
        let notice = ui.notices().next().unwrap();
        assert_eq!(notice.severity, Severity::Error);
        assert!(notice.text.contains("dance"));
        press(&mut ui, ":load ");
        assert_eq!(ui.key(KeyCode::Enter.into()), None);
    }

    #[test]
    fn notices_are_bounded_newest_first_and_cleaned() {
        let mut ui = Ui::default();
        for index in 0..10 {
            ui.notify(Notice::info(format!("event {index}\x1b[2J")));
        }
        let texts: Vec<_> = ui.notices().map(|n| n.text.clone()).collect();
        assert_eq!(texts.len(), MAX_NOTICES);
        assert!(texts[0].starts_with("event 9"));
        assert!(texts.iter().all(|t| !t.contains('\x1b')));
        assert_eq!(Notice::info("x".repeat(1000)).text.len(), MAX_NOTICE_CHARS);
    }

    #[test]
    fn long_drafts_keep_the_end_visible() {
        let mut ui = Ui::default();
        press(&mut ui, ":load /a/very/long/path.json");
        let footer = ui.footer(12);
        assert_eq!(footer.chars().count(), 12);
        assert_eq!(footer, "g/path.json▏");
    }
}
