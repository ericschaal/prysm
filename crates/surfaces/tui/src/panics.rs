//! Panic messages while the screen owns the terminal. The default hook would
//! write them into the alternate screen, which restoring the terminal erases,
//! so they are held and printed after the restore. A panic on any thread is
//! held: the screen keeps drawing until it is stopped.
use std::backtrace::{Backtrace, BacktraceStatus};
use std::panic::{self, PanicHookInfo};
use std::sync::{Mutex, MutexGuard, Once, PoisonError};

/// `Some` while the screen owns the terminal.
static HELD: Mutex<Option<Vec<String>>> = Mutex::new(None);

fn held() -> MutexGuard<'static, Option<Vec<String>>> {
    HELD.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Hold panic messages from now on. Installs the hook on first use, in
/// front of the one installed before it.
pub(crate) fn hold() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if keep(|| describe(info)) {
                return;
            }
            previous(info);
        }));
    });
    *held() = Some(Vec::new());
}

/// Stop holding; the messages held meanwhile, oldest first.
pub(crate) fn release() -> Vec<String> {
    held().take().unwrap_or_default()
}

/// Whether the message was held; it is built only then.
fn keep(message: impl FnOnce() -> String) -> bool {
    match held().as_mut() {
        Some(messages) => {
            messages.push(message());
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
    use std::process::Command;

    const CHILD: &str = "PRYSM_TUI_PANICS_CHILD";

    /// Runs [`held_panics_child`] alone in a child process: the hook is
    /// process-wide and would hold the panics of tests running beside it.
    #[test]
    fn a_panic_while_held_is_printed_only_on_release() {
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
        // Not held once released: the previous hook printed it.
        assert!(stderr.contains("after release"), "{stderr}");
        assert!(!stderr.contains("while held"), "{stderr}");
    }

    #[test]
    #[ignore = "run by a_panic_while_held_is_printed_only_on_release in a child process"]
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
        let messages = release();
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].starts_with("thread 'drawing' panicked at "),
            "{}",
            messages[0]
        );
        assert!(messages[0].ends_with(":\nwhile held"), "{}", messages[0]);
        assert!(release().is_empty());
        let unheld = std::thread::spawn(|| panic!("after release")).join();
        assert!(unheld.is_err());
    }
}
