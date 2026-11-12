//! `GET /metrics`: the engine's totals and a few snapshot fields in the
//! Prometheus text format 0.0.4, rendered by hand. Every label value is a
//! fixed word from a closed set, so nothing needs escaping.
use prysm_ports::{LightingTarget, SessionState, Snapshot};
use prysm_runtime::{Counters, Timing};
use std::fmt::Write;

pub(crate) const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

const TARGETS: [LightingTarget; 3] = [
    LightingTarget::Off,
    LightingTarget::Processed,
    LightingTarget::DeviceNative,
];

fn target_name(target: LightingTarget) -> &'static str {
    match target {
        LightingTarget::Off => "off",
        LightingTarget::Processed => "processed",
        LightingTarget::DeviceNative => "device_native",
    }
}

/// The session state without the faulted phase, to keep the label set small.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Stopped,
    Running,
    Native,
    Faulted,
}

impl State {
    const ALL: [Self; 4] = [Self::Stopped, Self::Running, Self::Native, Self::Faulted];

    fn name(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Running => "running",
            Self::Native => "native",
            Self::Faulted => "faulted",
        }
    }
}

impl From<SessionState> for State {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::Stopped => Self::Stopped,
            SessionState::Running => Self::Running,
            SessionState::Native => Self::Native,
            SessionState::Faulted(_) => Self::Faulted,
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Counter,
    Gauge,
    Summary,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Summary => "summary",
        }
    }
}

/// Collects the exposition text; `fmt::Write` into a `String` cannot fail.
struct Text(String);

impl Text {
    fn header(&mut self, name: &str, kind: Kind, help: &str) {
        let kind = kind.name();
        let _ = writeln!(self.0, "# HELP {name} {help}\n# TYPE {name} {kind}");
    }

    fn counter(&mut self, name: &str, help: &str, value: u64) {
        self.header(name, Kind::Counter, help);
        let _ = writeln!(self.0, "{name} {value}");
    }

    fn summary(&mut self, name: &str, help: &str, timing: Timing) {
        self.header(name, Kind::Summary, help);
        let _ = writeln!(
            self.0,
            "{name}_sum {}\n{name}_count {}",
            timing.total.as_secs_f64(),
            timing.count
        );
    }

    /// One 0/1 sample per label value, 1 for the current one.
    fn one_of<T: Copy + PartialEq>(
        &mut self,
        name: &str,
        help: &str,
        label: &str,
        all: &[T],
        current: T,
        value_name: impl Fn(T) -> &'static str,
    ) {
        self.header(name, Kind::Gauge, help);
        for &value in all {
            let _ = writeln!(
                self.0,
                "{name}{{{label}=\"{}\"}} {}",
                value_name(value),
                u8::from(value == current)
            );
        }
    }
}

pub(crate) fn render(counters: &Counters, snapshot: &Snapshot) -> String {
    let mut text = Text(String::with_capacity(2048));
    text.header("prysm_build_info", Kind::Gauge, "Prysm version; always 1.");
    let _ = writeln!(
        text.0,
        "prysm_build_info{{version=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION")
    );
    text.one_of(
        "prysm_target",
        "What the user asked the LEDs to do; 1 for the current target.",
        "target",
        &TARGETS,
        snapshot.target,
        target_name,
    );
    text.one_of(
        "prysm_session_state",
        "What the runtime is doing; 1 for the current state.",
        "state",
        &State::ALL,
        State::from(snapshot.session),
        State::name,
    );
    text.header(
        "prysm_settings_revision",
        Kind::Gauge,
        "Revision of the saved settings; starts at a random value per process.",
    );
    let _ = writeln!(text.0, "prysm_settings_revision {}", snapshot.revision.0);
    text.header(
        "prysm_input_fps",
        Kind::Gauge,
        "Frames per second from the source over the last second; 0 while no session runs.",
    );
    let fps = snapshot.input.map_or(0.0, |input| input.fps);
    let _ = writeln!(text.0, "prysm_input_fps {fps}");
    text.counter(
        "prysm_frames_received_total",
        "Frames taken from the capture source.",
        counters.frames_received,
    );
    text.counter(
        "prysm_frames_sent_total",
        "Frames whose colors were written to the output.",
        counters.frames_sent,
    );
    text.summary(
        "prysm_processing_seconds",
        "Time spent turning frames into colors.",
        counters.processing,
    );
    text.summary(
        "prysm_output_write_seconds",
        "Time spent in output writes, including keep-alive rewrites and failed writes.",
        counters.output_writes,
    );
    text.counter(
        "prysm_output_write_failures_total",
        "Output writes that failed; each ends lighting with a failure.",
        counters.output_write_failures,
    );
    text.counter(
        "prysm_source_opens_total",
        "Capture sources opened by starting lighting.",
        counters.source_opens,
    );
    text.counter(
        "prysm_output_opens_total",
        "Outputs opened to show colors: at the first frame and the first frame after a release.",
        counters.output_opens,
    );
    text.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::{Input, Revision, SessionPhase};
    use std::time::Duration;

    #[test]
    fn renders_the_exposition_format() {
        let counters = Counters {
            frames_received: 1200,
            frames_sent: 1199,
            processing: Timing {
                count: 1200,
                total: Duration::from_millis(2400),
            },
            output_writes: Timing {
                count: 1210,
                total: Duration::from_micros(605_500),
            },
            output_write_failures: 1,
            source_opens: 2,
            output_opens: 3,
        };
        let snapshot = Snapshot {
            target: LightingTarget::Processed,
            session: SessionState::Running,
            revision: Revision(42),
            input: Some(Input {
                fps: 29.9,
                receiving: true,
            }),
            ..Snapshot::default()
        };
        let expected = format!(
            "\
# HELP prysm_build_info Prysm version; always 1.
# TYPE prysm_build_info gauge
prysm_build_info{{version=\"{}\"}} 1
# HELP prysm_target What the user asked the LEDs to do; 1 for the current target.
# TYPE prysm_target gauge
prysm_target{{target=\"off\"}} 0
prysm_target{{target=\"processed\"}} 1
prysm_target{{target=\"device_native\"}} 0
# HELP prysm_session_state What the runtime is doing; 1 for the current state.
# TYPE prysm_session_state gauge
prysm_session_state{{state=\"stopped\"}} 0
prysm_session_state{{state=\"running\"}} 1
prysm_session_state{{state=\"native\"}} 0
prysm_session_state{{state=\"faulted\"}} 0
# HELP prysm_settings_revision Revision of the saved settings; starts at a random value per process.
# TYPE prysm_settings_revision gauge
prysm_settings_revision 42
# HELP prysm_input_fps Frames per second from the source over the last second; 0 while no session runs.
# TYPE prysm_input_fps gauge
prysm_input_fps 29.9
# HELP prysm_frames_received_total Frames taken from the capture source.
# TYPE prysm_frames_received_total counter
prysm_frames_received_total 1200
# HELP prysm_frames_sent_total Frames whose colors were written to the output.
# TYPE prysm_frames_sent_total counter
prysm_frames_sent_total 1199
# HELP prysm_processing_seconds Time spent turning frames into colors.
# TYPE prysm_processing_seconds summary
prysm_processing_seconds_sum 2.4
prysm_processing_seconds_count 1200
# HELP prysm_output_write_seconds Time spent in output writes, including keep-alive rewrites and failed writes.
# TYPE prysm_output_write_seconds summary
prysm_output_write_seconds_sum 0.6055
prysm_output_write_seconds_count 1210
# HELP prysm_output_write_failures_total Output writes that failed; each ends lighting with a failure.
# TYPE prysm_output_write_failures_total counter
prysm_output_write_failures_total 1
# HELP prysm_source_opens_total Capture sources opened by starting lighting.
# TYPE prysm_source_opens_total counter
prysm_source_opens_total 2
# HELP prysm_output_opens_total Outputs opened to show colors: at the first frame and the first frame after a release.
# TYPE prysm_output_opens_total counter
prysm_output_opens_total 3
",
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(render(&counters, &snapshot), expected);
    }

    #[test]
    fn a_faulted_session_is_one_state_and_no_session_has_no_rate() {
        let snapshot = Snapshot {
            session: SessionState::Faulted(SessionPhase::ClosingSource),
            ..Snapshot::default()
        };
        let text = render(&Counters::default(), &snapshot);
        assert!(text.contains("prysm_session_state{state=\"faulted\"} 1\n"));
        assert!(text.contains("prysm_target{target=\"off\"} 1\n"));
        assert!(text.contains("prysm_input_fps 0\n"));
        assert!(text.contains("prysm_processing_seconds_sum 0\n"));
    }
}
