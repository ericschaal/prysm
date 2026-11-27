//! Panic messages while the screen owns the terminal. The default hook would
//! write them into the alternate screen, which restoring the terminal erases,
//! so they are held and printed after the restore. A panic on any thread is
//! held: the screen keeps drawing until it is stopped.
use std::backtrace::{Backtrace, BacktraceStatus};
use std::io::Write;
use std::panic::{self, PanicHookInfo};
use std::sync::{Mutex, MutexGuard, Once, PoisonError};

/// Messages kept; later panics are only counted.
const KEPT: usize = 8;

/// The panics while the terminal was held.
#[derive(Debug, Default)]
pub(crate) struct Held {
    /// The first [`KEPT`], oldest first.
    messages: Vec<String>,
    /// How many more there were.
    more: usize,
}

impl Held {
    fn keep(&mut self, message: String) {
        if self.messages.len() < KEPT {
            self.messages.push(message);
        } else {
            self.more += 1;
        }
    }

    /// Writes them to `out`; a failed write is ignored, as the terminal may
    /// be gone.
    pub(crate) fn write_to(&self, out: &mut impl Write) {
        for message in &self.messages {
            let _ = writeln!(out, "{message}");
        }
        if self.more > 0 {
            let _ = writeln!(out, "{} more panics", self.more);
        }
    }
}

/// `Some` while the screen owns the terminal.
static HELD: Mutex<Option<Held>> = Mutex::new(None);

fn held() -> MutexGuard<'static, Option<Held>> {
    HELD.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Hold panic messages from now on, keeping those already held. Installs
/// the hook on first use, in front of the one installed before it.
pub(crate) fn hold() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            // Described without the lock, so a panic inside cannot deadlock.
            if !(held().is_some() && keep(describe(info))) {
                previous(info);
            }
        }));
    });
    held().get_or_insert_with(Held::default);
}

/// Stop holding; the panics held meanwhile.
pub(crate) fn release() -> Held {
    held().take().unwrap_or_default()
}

/// Whether the message was held.
fn keep(message: String) -> bool {
    match held().as_mut() {
        Some(held) => {
            held.keep(message);
            true
        }
        None => false,
    }
}

/// What the default hook prints: thread, location, message and, when
/// `RUST_BACKTRACE` asks for one, the backtrace.
fn describe(info: &PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("<unnamed>");
    let location = info
        .location()
        .map_or_else(String::new, |location| format!(" at {location}"));
    let payload = info.payload_as_str().unwrap_or("Box<dyn Any>");
    let mut message = format!("thread '{name}' panicked{location}:\n{payload}");
    let backtrace = Backtrace::capture();
    if backtrace.status() == BacktraceStatus::Captured {
        message.push_str(&format!("\nstack backtrace:\n{backtrace}"));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::process::Command;

    #[test]
    fn the_first_messages_are_kept_and_the_rest_counted() {
        let mut held = Held::default();
        for n in 0..KEPT + 2 {
            held.keep(format!("panic {n}"));
        }
        let mut out = Vec::new();
        held.write_to(&mut out);
        let out = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), KEPT + 1);
        assert_eq!(lines[0], "panic 0");
        assert_eq!(lines[KEPT - 1], format!("panic {}", KEPT - 1));
        assert_eq!(lines[KEPT], "2 more panics");
    }

    #[test]
    fn writing_to_a_terminal_that_is_gone_does_not_panic() {
        struct Gone;
        impl Write for Gone {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from_raw_os_error(5))
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::from_raw_os_error(5))
            }
        }
        let mut held = Held::default();
        held.keep("panic".into());
        held.write_to(&mut Gone);
    }

    const CHILD: &str = "PRYSM_TUI_PANICS_CHILD";

    /// Runs [`held_panics_child`] alone in a child process: the hook is
    /// process-wide and would hold the panics of tests running beside it.
    #[test]
    fn the_hook_holds_panics_until_release_then_passes_them_on() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "panics::tests::held_panics_child",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env_remove("RUST_BACKTRACE")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(
            stdout.contains("1 passed"),
            "the child test did not run: {stdout}"
        );
        assert!(stderr.contains("after release"), "{stderr}");
        assert!(!stderr.contains("while held"), "{stderr}");
    }

    #[test]
    #[ignore = "run by the_hook_holds_panics_until_release_then_passes_them_on in a child process"]
    fn held_panics_child() {
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        hold();
        let panicked = std::thread::Builder::new()
            .name("drawing".into())
            .spawn(|| panic!("while held"))
            .unwrap()
            .join();
        assert!(panicked.is_err());
        hold();
        let held = release();
        assert_eq!(held.messages.len(), 1, "{held:?}");
        assert_eq!(held.more, 0);
        let message = &held.messages[0];
        assert!(
            message.starts_with("thread 'drawing' panicked at "),
            "{message}"
        );
        assert!(message.ends_with(":\nwhile held"), "{message}");
        assert!(release().messages.is_empty());
        let unheld = std::thread::spawn(|| panic!("after release")).join();
        assert!(unheld.is_err());
    }
}
