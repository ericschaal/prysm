//! Command-line editing only. Parsing and application control live outside the screen.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;

pub(super) struct CommandLine {
    sender: mpsc::Sender<String>,
    draft: Option<String>,
    message: String,
}
impl CommandLine {
    pub fn new(sender: mpsc::Sender<String>) -> Self {
        Self {
            sender,
            draft: None,
            message: String::new(),
        }
    }

    pub fn key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release
            || key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }
        let Some(draft) = &mut self.draft else {
            if key.code == KeyCode::Char(':') {
                self.draft = Some(String::new());
                self.message.clear();
            }
            return;
        };
        match key.code {
            KeyCode::Esc => self.draft = None,
            KeyCode::Backspace => {
                draft.pop();
            }
            KeyCode::Char(c) if !c.is_control() && draft.len() + c.len_utf8() <= 4096 => {
                draft.push(c)
            }
            KeyCode::Enter => {
                if draft.trim().is_empty() {
                    self.draft = None;
                    return;
                }
                match self.sender.try_send(draft.clone()) {
                    Ok(()) => {
                        self.draft = None;
                        self.message =
                            "Queued; acceptance and completion appear in diagnostics".into();
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        self.message = "Command queue full; retry Enter".into()
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        self.message = "Commands unavailable".into()
                    }
                }
            }
            _ => {}
        }
    }

    pub fn text(&self, width: usize) -> String {
        let text = match &self.draft {
            Some(draft) => format!(":{draft}▏  {}", self.message),
            None if !self.message.is_empty() => {
                format!("{} · : command · Ctrl-C exit", self.message)
            }
            None => ": command: on | off | load <path> · Ctrl-C exit".into(),
        };
        // Keep the editing end visible on small terminals without splitting UTF-8.
        let skip = text.chars().count().saturating_sub(width);
        text.chars().skip(skip).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn type_text(line: &mut CommandLine, text: &str) {
        for c in text.chars() {
            line.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }
    #[test]
    fn scrolling_is_ignored_and_only_enter_submits_edited_text() {
        let (sender, mut receiver) = mpsc::channel(1);
        let mut line = CommandLine::new(sender);
        type_text(&mut line, "ignored");
        line.key(KeyCode::Up.into());
        assert!(receiver.try_recv().is_err());
        type_text(&mut line, ":load /tmp/caféx");
        line.key(KeyCode::Backspace.into());
        line.key(KeyCode::Enter.into());
        assert_eq!(receiver.try_recv().unwrap(), "load /tmp/café");
        type_text(&mut line, ":off");
        line.key(KeyCode::Esc.into());
        line.key(KeyCode::Enter.into());
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn full_queue_preserves_draft_for_explicit_retry() {
        let (sender, mut receiver) = mpsc::channel(1);
        let mut line = CommandLine::new(sender);
        type_text(&mut line, ":on");
        line.key(KeyCode::Enter.into());
        type_text(&mut line, ":off");
        line.key(KeyCode::Enter.into());
        assert!(line.text(100).contains("queue full"));
        assert_eq!(receiver.try_recv().unwrap(), "on");
        line.key(KeyCode::Enter.into());
        assert_eq!(receiver.try_recv().unwrap(), "off");
    }
}
