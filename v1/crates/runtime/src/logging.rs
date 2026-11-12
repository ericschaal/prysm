//! The hosts' log: `tracing` events, which the shared capture and processing
//! crates report through, as one line each on stderr or in a file, in the
//! style of the hosts' own status lines:
//!
//! ```text
//! prysm-headless: warn prysm_capture::linux: capture buffer cleanup failed error=...
//! ```
//!
//! `RUST_LOG` takes `tracing_subscriber::filter::Targets` syntax, such as
//! `debug` or `warn,prysm_capture=debug`. Target prefixes match literally, so
//! `prysm` covers `prysm_capture`, `prysm_processor` and `prysm::stats`.
use std::fs::File;
use std::io;
use std::sync::Mutex;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{self, FmtContext, FormatEvent, FormatFields, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

/// Used when `RUST_LOG` is unset, empty or invalid: dependencies at `warn`,
/// Prysm's crates at `info`.
pub const DEFAULT_FILTER: &str = "warn,prysm=info";

/// Where log lines go. Never stdout: the TV workers' stdout is the supervisor
/// protocol.
#[derive(Debug)]
pub enum Logs {
    Stderr,
    /// Appended to, for hosts whose terminal is taken by a full-screen surface.
    File(File),
}

/// Install the process-wide subscriber, prefixing lines with `name`. An
/// invalid `RUST_LOG` falls back to [`DEFAULT_FILTER`] and is reported as
/// the first line. A second call does nothing.
pub fn init(name: &'static str, logs: Logs) {
    let (filter, invalid) = filter(std::env::var("RUST_LOG").ok().as_deref());
    let installed = match logs {
        Logs::Stderr => subscriber(name, filter, io::stderr).try_init(),
        Logs::File(file) => subscriber(name, filter, Mutex::new(file)).try_init(),
    };
    if let (Ok(()), Some(invalid)) = (installed, invalid) {
        tracing::warn!(target: "prysm", "ignoring RUST_LOG: {invalid}");
    }
}

/// The filter for a `RUST_LOG` value, and why it was rejected if it was.
fn filter(rust_log: Option<&str>) -> (Targets, Option<String>) {
    let default = || DEFAULT_FILTER.parse().expect("valid default filter");
    match rust_log.map(str::trim).filter(|value| !value.is_empty()) {
        None => (default(), None),
        Some(value) => match value.parse() {
            Ok(filter) => (filter, None),
            Err(error) => (default(), Some(format!("{value:?}: {error}"))),
        },
    }
}

fn subscriber<W>(name: &'static str, filter: Targets, writer: W) -> impl Subscriber + Send + Sync
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().event_format(Line(name)).with_writer(writer))
}

/// `<name>: <level> <target>: <message> <field>=<value>...`; spans are not shown.
struct Line(&'static str);

impl<S, N> FormatEvent<S, N> for Line
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        context: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let metadata = event.metadata();
        let level = match *metadata.level() {
            Level::ERROR => "error",
            Level::WARN => "warn",
            Level::INFO => "info",
            Level::DEBUG => "debug",
            Level::TRACE => "trace",
        };
        write!(writer, "{}: {level} {}: ", self.0, metadata.target())?;
        context
            .field_format()
            .format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::Arc;

    /// Lines written under `rust_log` while `emit` runs.
    fn capture(rust_log: Option<&str>, emit: impl FnOnce()) -> Vec<String> {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = {
            let buffer = buffer.clone();
            move || Buffer(buffer.clone())
        };
        let (filter, invalid) = filter(rust_log);
        assert_eq!(invalid, None);
        tracing::subscriber::with_default(subscriber("prysm-test", filter, writer), emit);
        let bytes = buffer.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

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

    fn events() {
        tracing::warn!(target: "prysm_capture::linux", error = "No such device", "cannot open");
        tracing::info!(target: "prysm_processor", "configured");
        tracing::debug!(target: "prysm_capture::linux", "frame");
        tracing::info!(target: "hyper::proto", "connection");
        tracing::warn!(target: "hyper::proto", "reset");
    }

    #[test]
    fn default_filter_shows_prysm_info_and_dependency_warnings_on_one_line_each() {
        assert_eq!(
            capture(None, events),
            [
                r#"prysm-test: warn prysm_capture::linux: cannot open error="No such device""#,
                "prysm-test: info prysm_processor: configured",
                "prysm-test: warn hyper::proto: reset",
            ]
        );
        assert_eq!(capture(Some(" "), events).len(), 3);
    }

    #[test]
    fn rust_log_filters_by_target_and_level() {
        assert_eq!(
            capture(Some("error,prysm_capture=debug"), events),
            [
                r#"prysm-test: warn prysm_capture::linux: cannot open error="No such device""#,
                "prysm-test: debug prysm_capture::linux: frame",
            ]
        );
        assert!(capture(Some("prysm_capture=error"), events).is_empty());
        let (_, invalid) = filter(Some("prysm_capture=loud"));
        assert!(invalid.unwrap().starts_with(r#""prysm_capture=loud": "#));
    }

    const CHILD: &str = "PRYSM_LOGGING_TEST_CHILD";

    /// Runs this test binary again with only [`child_process_logs`], which
    /// installs the real subscriber: the warning reaches stderr, filtered by
    /// `RUST_LOG`, and nothing reaches stdout.
    #[test]
    fn installed_subscriber_writes_to_stderr_only() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "logging::tests::child_process_logs",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("RUST_LOG", "warn,prysm_processor=error")
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(
            stdout.contains("1 passed"),
            "the child test did not run: {stdout}"
        );
        assert!(!stdout.contains("prysm-child"), "{stdout}");
        assert!(
            stderr.contains("prysm-child: warn prysm_capture::linux: cannot open\n"),
            "{stderr}"
        );
        assert!(!stderr.contains("filtered"), "{stderr}");
    }

    #[test]
    fn child_process_logs() {
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        init("prysm-child", Logs::Stderr);
        tracing::warn!(target: "prysm_capture::linux", "cannot open");
        tracing::warn!(target: "prysm_processor", "filtered");
        tracing::info!(target: "prysm_capture::linux", "filtered");
    }
}
