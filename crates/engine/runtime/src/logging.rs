//! Where the hosts' log lines go: [`prysm_logging`]'s subscriber on stderr,
//! on stderr while a full-screen surface does not hold the terminal, or in a
//! file.
use std::fs::File;
use std::io;
use std::sync::Mutex;
use tokio::sync::watch;
use tracing_subscriber::fmt::MakeWriter;

/// Where log lines go. Never stdout: the TV workers' stdout is the supervisor
/// protocol.
#[derive(Debug)]
pub enum Logs {
    Stderr,
    /// Stderr, except while `taken` holds `true`: a full-screen surface then
    /// draws on the terminal stderr writes to, and lines are dropped.
    SharedStderr {
        taken: watch::Receiver<bool>,
    },
    /// Appended to, for hosts whose terminal is taken by a full-screen surface.
    File(File),
}

/// Install the process-wide subscriber ([`prysm_logging::init`]), prefixing
/// lines with `name`. A second call does nothing.
pub fn init(name: &'static str, logs: Logs) {
    match logs {
        Logs::Stderr => prysm_logging::init(name, io::stderr),
        Logs::SharedStderr { taken } => {
            prysm_logging::init(
                name,
                Shared {
                    taken,
                    out: io::stderr,
                },
            );
        }
        Logs::File(file) => prysm_logging::init(name, Mutex::new(file)),
    }
}

/// [`Logs::SharedStderr`]'s writer: `out` (stderr) unless `taken`.
struct Shared<M> {
    taken: watch::Receiver<bool>,
    out: M,
}

/// Holds the borrow while the line is written, so the terminal cannot be
/// taken in the middle of it.
struct SharedLine<'a, W> {
    taken: watch::Ref<'a, bool>,
    out: W,
}

impl<W: io::Write> io::Write for SharedLine<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if *self.taken {
            Ok(bytes.len())
        } else {
            self.out.write(bytes)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for Shared<M> {
    type Writer = SharedLine<'a, M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLine {
            taken: self.taken.borrow(),
            out: self.out.make_writer(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Arc;

    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn shared_stderr_writes_lines_while_free_and_drops_them_while_taken() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let out = {
            let buffer = buffer.clone();
            move || Buffer(buffer.clone())
        };
        let taken = watch::Sender::new(false);
        let shared = Shared {
            taken: taken.subscribe(),
            out,
        };
        let line = |text: &str| shared.make_writer().write_all(text.as_bytes()).unwrap();
        line("free\n");
        taken.send_replace(true);
        line("taken\n");
        taken.send_replace(false);
        line("free again\n");
        let text = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert_eq!(text, "free\nfree again\n");
    }
}
