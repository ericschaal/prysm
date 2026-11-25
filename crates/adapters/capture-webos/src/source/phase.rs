//! The native thread's decisions as a pure state machine: an event and the
//! current time in, the next state and the actions to perform out. No clock,
//! no logging, no backend, no worker; every rule is table-tested.
use super::{Acquired, Fault, Limits, Recovery};
use crate::{Readiness, Video};
use prysm_core::Frame;
use prysm_ports::Error;
use std::time::{Duration, Instant};

/// Which tap the native thread holds. The two never run together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Video,
    Ui,
}

/// Whether the TV UI tap works in this source, and how its next fault is
/// logged. A UI fault that cannot heal makes it unsupported until the next
/// source; any other stops the tries until the video or its context
/// changes, never on a timer: a UI tap can fail for good (on the bench TV, a
/// worker that died with the UI tap open left the graphics driver's lock
/// held, and every later UI open failed until a reboot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UiSupport {
    Untried,
    Works,
    /// Failed in a way that may pass: not opened until the video or its
    /// context changes.
    Failed,
    /// May be opened again after a failure; its next fault is no news.
    Again,
    /// Cannot work in this process: never opened again in this source.
    Unsupported,
}

/// Whether a fault is the first since the last frame (a warning) or a
/// repeat (debug).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Occurrence {
    First,
    Again,
}

/// A refusal period of the held driver: from an initialisation the driver
/// refused to the first answer that is not a refusal, or a change of tap
/// kind. The pulls are told refused meanwhile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Refusal {
    since: Instant,
    /// Initialisations of the created driver in this period, the refused
    /// one that started it included.
    initializations: u32,
}

/// Where a tap's period stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    /// No tap: open at `at`. A refusal runs on when its tap was released at
    /// a change of context without being created-only, or failed to reopen.
    Closed {
        at: Instant,
        refusal: Option<Refusal>,
    },
    /// The tap is being released after a fault; it reopens `wait` after the
    /// release completes.
    Releasing { wait: Duration },
    /// A tap with no deadline running: acquired at the pace.
    Running,
    /// The driver answered not ready since `since`; acquired again at `at`.
    Preparing { since: Instant, at: Instant },
    /// Initialised, with no buffer since `since`.
    Pending { since: Instant },
    /// The driver refused: initialised again at the next change of the
    /// video signal, since nothing tells when the TV would allow it sooner.
    Refused(Refusal),
    /// The video signal changed since the refusal: initialised once now.
    Reinitialising(Refusal),
}

/// The tap the native thread's current period is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    /// No tap wanted: until the video signal changes.
    Idle,
    Video {
        stage: Stage,
        /// How the next video fault of this period is logged.
        next_fault: Occurrence,
    },
    Ui(Stage),
}

/// The signal last seen, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Seen {
    readiness: Readiness,
    at: Instant,
}

/// The native thread's whole decision state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct State {
    phase: Phase,
    /// Outlives a phase: kept for the whole source.
    ui: UiSupport,
    /// `None` before the first signal, which counts as a change.
    seen: Option<Seen>,
    /// The paced clock: when the step after the next is due.
    tick: Instant,
}

/// What the machine is told in a run of the loop.
#[derive(Debug)]
pub(super) enum Event {
    /// The signal read has a new sequence. `created_only`: the held tap's
    /// driver is created and not initialised, as the tap says now (`false`
    /// without a tap).
    Signal {
        readiness: Readiness,
        created_only: bool,
    },
    /// The tap [`Action::Open`] asked for opened.
    Opened,
    OpenFailed(Fault),
    /// An acquisition answered; `initializing`: the driver was created-only
    /// before it, as the tap said.
    Acquired {
        answer: Result<Acquired, Fault>,
        initializing: bool,
    },
    /// The [`Action::Release`] after a fault completed.
    Released,
    /// A sleep or an idle ended without a change of the signal.
    Woke,
    Stop,
}

/// What the shell performs, in order. A failing release fails the source.
#[derive(Debug, Clone)]
pub(super) enum Action {
    Open(Kind),
    Acquire,
    /// Release the held tap.
    Release,
    Deliver(Frame),
    /// Tell the pulls whether the TV refuses.
    Refused(bool),
    Note(Note),
    /// Sleep up to this long, or until the signal changes or a stop.
    Sleep(Duration),
    /// Sleep until the signal changes or a stop.
    Idle,
    /// Publish this failure and end; a held tap is then released.
    Fail(Error),
}

/// What a video acquisition says of the driver's initialisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Answer {
    /// `vtCapture_init` refused (status 17).
    Refused,
    /// Not ready yet (status 1).
    NotReady,
    /// Initialised: a frame or a pending buffer.
    Accepted,
    /// A native failure.
    Failed,
}

/// A log line, decided here and written by the shell. Each variant is one
/// log line of the native thread; an [`Occurrence`] picks a warning or debug.
#[derive(Debug, Clone)]
pub(super) enum Note {
    /// debug: "video signal changed".
    SignalChanged(Readiness),
    /// info: "video changed".
    VideoChanged(Video),
    /// info: "the video signal changed while the TV refuses; initializing
    /// the driver again".
    ChangedWhileRefused { video: Video, generation: u64 },
    /// debug: "webOS video initialization answered".
    InitializationAnswered {
        answer: Answer,
        since_change_ms: u128,
    },
    /// info: "the TV doesn’t let Prysm capture this video; …"
    /// (`initializations = 1`).
    Refused { since_change_ms: u128 },
    /// info: "the TV refused again".
    RefusedAgain {
        initializations: u32,
        since_change_ms: u128,
    },
    /// debug: "the refusal ended with the video".
    RefusalEndedWithVideo {
        initializations: u32,
        elapsed_ms: u128,
    },
    /// debug: "the refusal ended with a driver that is preparing".
    RefusalEndedPreparing {
        initializations: u32,
        elapsed_ms: u128,
    },
    /// info: "the TV lets Prysm capture this video now".
    CaptureAllowed {
        initializations: u32,
        elapsed_ms: u128,
    },
    /// debug: "the refusal ended with a native failure".
    RefusalEndedFailure {
        initializations: u32,
        elapsed_ms: u128,
    },
    /// info: "capturing the TV UI while there is no video".
    UiCapturing { width: u32, height: u32 },
    /// warn or debug: "TV UI capture failed; tried again once the video
    /// signal changes".
    UiFailed {
        error: Error,
        occurrence: Occurrence,
    },
    /// warn or debug: "TV UI capture is unsupported; no frames while there
    /// is no video".
    UiUnsupported {
        error: Error,
        occurrence: Occurrence,
    },
    /// warn or debug: "webOS video capture failed; retrying".
    VideoRetrying {
        error: Error,
        retry_in: Duration,
        occurrence: Occurrence,
    },
}

/// What the decisions depend on besides the state.
#[derive(Debug, Clone, Copy)]
pub(super) struct Rules {
    pub limits: Limits,
    pub include_ui: bool,
    /// Between two steps.
    pub period: Duration,
}

/// The state after an event, and what to perform.
#[derive(Debug)]
pub(super) struct Step {
    pub state: State,
    pub actions: Vec<Action>,
}

impl UiSupport {
    /// After a TV UI fault.
    fn after_fault(recovery: Recovery) -> Self {
        match recovery {
            Recovery::Never => Self::Unsupported,
            Recovery::Reopen => Self::Failed,
        }
    }

    /// After the video or its context changed: a failed UI may be tried
    /// again.
    fn after_change(self) -> Self {
        match self {
            Self::Failed => Self::Again,
            other => other,
        }
    }

    fn may_open(self) -> bool {
        matches!(self, Self::Untried | Self::Works | Self::Again)
    }

    /// How a fault in this state is logged: one since the last UI frame is
    /// no news.
    fn occurrence(self) -> Occurrence {
        match self {
            Self::Untried | Self::Works => Occurrence::First,
            Self::Failed | Self::Again | Self::Unsupported => Occurrence::Again,
        }
    }
}

/// When a video tap reopens after a recoverable fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reopen {
    /// At once: the fault is a stall, which already waited
    /// [`Limits::pending`], so a reopen cannot spin.
    AtOnce,
    /// After [`Limits::retry`], so a tap that fails at once is not reopened
    /// in a tight loop.
    AfterRetry,
}

/// After a video fault: how long until the tap reopens, or `None` when the
/// source fails.
fn video_wait(recovery: Recovery, reopen: Reopen, limits: &Limits) -> Option<Duration> {
    match (recovery, reopen) {
        (Recovery::Never, _) => None,
        (Recovery::Reopen, Reopen::AtOnce) => Some(Duration::ZERO),
        (Recovery::Reopen, Reopen::AfterRetry) => Some(limits.retry),
    }
}

/// The tap for `video`: video while it is available or unknown, the TV UI
/// without video while `include_ui` and the UI may open, else none.
fn wanted(video: Video, include_ui: bool, ui: UiSupport) -> Option<Kind> {
    match video {
        Video::Available | Video::Unknown => Some(Kind::Video),
        Video::Unavailable if include_ui && ui.may_open() => Some(Kind::Ui),
        Video::Unavailable => None,
    }
}

impl Answer {
    fn of(acquired: &Result<Acquired, Fault>) -> Self {
        match acquired {
            Ok(Acquired::Refused) => Self::Refused,
            Ok(Acquired::NotReady) => Self::NotReady,
            Ok(Acquired::Frame(_) | Acquired::Pending) => Self::Accepted,
            Err(_) => Self::Failed,
        }
    }
}

impl Stage {
    fn holds_tap(self) -> bool {
        !matches!(self, Self::Closed { .. } | Self::Releasing { .. })
    }

    fn refusal(self) -> Option<Refusal> {
        match self {
            Self::Closed { refusal, .. } => refusal,
            Self::Refused(refusal) | Self::Reinitialising(refusal) => Some(refusal),
            Self::Releasing { .. }
            | Self::Running
            | Self::Preparing { .. }
            | Self::Pending { .. } => None,
        }
    }
}

impl Phase {
    /// A new period for `kind` at `stage`: its first video fault is news.
    fn period(kind: Option<Kind>, stage: Stage) -> Self {
        match kind {
            None => Self::Idle,
            Some(Kind::Video) => Self::Video {
                stage,
                next_fault: Occurrence::First,
            },
            Some(Kind::Ui) => Self::Ui(stage),
        }
    }

    fn kind(self) -> Option<Kind> {
        match self {
            Self::Idle => None,
            Self::Video { .. } => Some(Kind::Video),
            Self::Ui(_) => Some(Kind::Ui),
        }
    }

    fn stage(self) -> Option<Stage> {
        match self {
            Self::Idle => None,
            Self::Video { stage, .. } | Self::Ui(stage) => Some(stage),
        }
    }

    /// The same period at `stage`; idle stays idle.
    fn at(self, stage: Stage) -> Self {
        match self {
            Self::Idle => Self::Idle,
            Self::Video { next_fault, .. } => Self::Video { stage, next_fault },
            Self::Ui(_) => Self::Ui(stage),
        }
    }
}

impl State {
    /// A source starting at `now`: nothing wanted until the first signal.
    pub(super) fn new(now: Instant) -> Self {
        Self {
            phase: Phase::Idle,
            ui: UiSupport::Untried,
            seen: None,
            tick: now,
        }
    }

    pub(super) fn phase(&self) -> Phase {
        self.phase
    }

    /// The sequence of the signal last seen, so the shell tells only new
    /// ones.
    pub(super) fn sequence(&self) -> Option<u64> {
        self.seen.map(|seen| seen.readiness.sequence)
    }
}

/// The state after `event` at `now`, and what to perform.
pub(super) fn next(mut state: State, event: Event, now: Instant, rules: &Rules) -> Step {
    let mut actions = Vec::new();
    let out = &mut actions;
    match event {
        Event::Signal {
            readiness,
            created_only,
        } => {
            let context_changed = state.signal(readiness, now, out);
            state.period(context_changed, created_only, now, rules, out);
            state.decide(now, out);
        }
        Event::Woke => {
            state.period(false, false, now, rules, out);
            state.decide(now, out);
        }
        Event::Opened => state.opened(out),
        Event::OpenFailed(fault) => state.recover(fault, Reopen::AfterRetry, now, rules, out),
        Event::Acquired {
            answer,
            initializing,
        } => state.acquired(answer, initializing, now, rules, out),
        Event::Released => {
            if let Some(Stage::Releasing { wait }) = state.phase.stage() {
                state.phase = state.phase.at(Stage::Closed {
                    at: now + wait,
                    refusal: None,
                });
            }
            state.pace(now, rules, out);
        }
        Event::Stop => {
            if state.phase.stage().is_some_and(Stage::holds_tap) {
                out.push(Action::Release);
            }
            state.phase = Phase::Idle;
        }
    }
    Step { state, actions }
}

/// Whole milliseconds from `since` to `now`.
fn millis(since: Instant, now: Instant) -> u128 {
    now.saturating_duration_since(since).as_millis()
}

impl State {
    /// Whole milliseconds from the last change of the video signal to `now`.
    fn since_change(&self, now: Instant) -> u128 {
        self.seen.map_or(0, |seen| millis(seen.at, now))
    }

    /// The signal changed to `readiness` (the first counts as a change of
    /// video). Any change lets a refused driver be initialised once more;
    /// only a change of video or context lets a failed TV UI be tried
    /// again. Returns whether the context changed.
    fn signal(&mut self, readiness: Readiness, now: Instant, out: &mut Vec<Action>) -> bool {
        let before = self.seen.replace(Seen { readiness, at: now });
        let Readiness {
            video, generation, ..
        } = readiness;
        out.push(Action::Note(Note::SignalChanged(readiness)));
        let video_changed = before.is_none_or(|before| before.readiness.video != video);
        if video_changed {
            out.push(Action::Note(Note::VideoChanged(video)));
        }
        let context_changed =
            before.is_some_and(|before| before.readiness.generation != generation);
        if video_changed || context_changed {
            self.ui = self.ui.after_change();
        }
        if let Some(stage) = self.phase.stage()
            && stage.refusal().is_some()
        {
            if let Stage::Refused(refusal) = stage {
                self.phase = self.phase.at(Stage::Reinitialising(refusal));
            }
            out.push(Action::Note(Note::ChangedWhileRefused {
                video,
                generation,
            }));
        }
        context_changed
    }

    /// A change of the wanted tap kind, or of the video context, starts a
    /// new period, due at once. A created-only driver holds no video
    /// context and its release can hang, so only a change of kind releases
    /// it; a change of kind also ends a refusal.
    fn period(
        &mut self,
        context_changed: bool,
        created_only: bool,
        now: Instant,
        rules: &Rules,
        out: &mut Vec<Action>,
    ) {
        let video = self
            .seen
            .map(|seen| seen.readiness.video)
            .unwrap_or_default();
        let kind = wanted(video, rules.include_ui, self.ui);
        if kind == self.phase.kind() && !context_changed {
            return;
        }
        let stage = self.phase.stage();
        let held = stage.is_some_and(Stage::holds_tap);
        let refusal = stage.and_then(Stage::refusal);
        let stage = if kind != self.phase.kind() {
            if held {
                out.push(Action::Release);
            }
            if let Some(refusal) = refusal {
                out.push(Action::Refused(false));
                out.push(Action::Note(Note::RefusalEndedWithVideo {
                    initializations: refusal.initializations,
                    elapsed_ms: millis(refusal.since, now),
                }));
            }
            Stage::Closed {
                at: now,
                refusal: None,
            }
        } else if held && created_only {
            refusal.map_or(Stage::Running, Stage::Reinitialising)
        } else {
            if held {
                out.push(Action::Release);
            }
            Stage::Closed { at: now, refusal }
        };
        self.phase = Phase::period(kind, stage);
    }

    /// Idle without a tap wanted or while refused; wait for a deadline not
    /// reached; else open or acquire.
    fn decide(&self, now: Instant, out: &mut Vec<Action>) {
        let Some(kind) = self.phase.kind() else {
            out.push(Action::Idle);
            return;
        };
        match self.phase.stage() {
            None | Some(Stage::Refused(_)) => out.push(Action::Idle),
            Some(Stage::Closed { at, .. } | Stage::Preparing { at, .. }) if now < at => {
                out.push(Action::Sleep(at - now));
            }
            Some(Stage::Closed { .. }) => out.push(Action::Open(kind)),
            // Only [`Event::Released`] moves on.
            Some(Stage::Releasing { .. }) => {}
            Some(
                Stage::Running
                | Stage::Preparing { .. }
                | Stage::Pending { .. }
                | Stage::Reinitialising(_),
            ) => out.push(Action::Acquire),
        }
    }

    /// The tap opened: acquire at once, in the same step.
    fn opened(&mut self, out: &mut Vec<Action>) {
        if let Some(Stage::Closed { refusal, .. }) = self.phase.stage() {
            self.phase = self
                .phase
                .at(refusal.map_or(Stage::Running, Stage::Reinitialising));
            out.push(Action::Acquire);
        }
    }
}

impl State {
    /// Act on one acquisition, an `initializing` one if the driver was
    /// created-only: a refusal starts, goes on or ends; then publish a
    /// frame, time a pending or preparing driver, or recover from a fault.
    fn acquired(
        &mut self,
        answer: Result<Acquired, Fault>,
        initializing: bool,
        now: Instant,
        rules: &Rules,
        out: &mut Vec<Action>,
    ) {
        let kind = Answer::of(&answer);
        if initializing {
            out.push(Action::Note(Note::InitializationAnswered {
                answer: kind,
                since_change_ms: self.since_change(now),
            }));
        }
        let Some(stage) = self.phase.stage() else {
            return;
        };
        let refusal = stage.refusal();
        if kind == Answer::Refused {
            self.refused(refusal, now, out);
        } else if let Some(refusal) = refusal {
            out.push(Action::Refused(false));
            let initializations = refusal.initializations + 1;
            let elapsed_ms = millis(refusal.since, now);
            out.push(Action::Note(match kind {
                Answer::NotReady => Note::RefusalEndedPreparing {
                    initializations,
                    elapsed_ms,
                },
                Answer::Accepted => Note::CaptureAllowed {
                    initializations,
                    elapsed_ms,
                },
                // A refusal goes on above and never reaches here.
                Answer::Refused | Answer::Failed => Note::RefusalEndedFailure {
                    initializations,
                    elapsed_ms,
                },
            }));
        }
        match answer {
            Ok(Acquired::Frame(frame)) => {
                self.phase = match self.phase.at(Stage::Running) {
                    Phase::Video { stage, .. } => Phase::Video {
                        stage,
                        next_fault: Occurrence::First,
                    },
                    other => other,
                };
                // The first UI frame is reported once.
                if matches!(self.phase, Phase::Ui(_)) && self.ui != UiSupport::Works {
                    self.ui = UiSupport::Works;
                    out.push(Action::Note(Note::UiCapturing {
                        width: frame.width(),
                        height: frame.height(),
                    }));
                }
                out.push(Action::Deliver(frame));
            }
            Ok(Acquired::Pending) => {
                let since = match stage {
                    Stage::Pending { since } => since,
                    _ => now,
                };
                if now.duration_since(since) >= rules.limits.pending {
                    let stalled = Fault::recoverable(Error::other(format!(
                        "webOS video produced no frame for {:?}",
                        rules.limits.pending
                    )));
                    return self.recover(stalled, Reopen::AtOnce, now, rules, out);
                }
                self.phase = self.phase.at(Stage::Pending { since });
            }
            Ok(Acquired::NotReady) => {
                let since = match stage {
                    Stage::Preparing { since, .. } => since,
                    _ => now,
                };
                if now.duration_since(since) >= rules.limits.preparation {
                    let error = Error::other(format!(
                        "webOS video was not ready within {:?}",
                        rules.limits.preparation
                    ));
                    return self.fail(error, out);
                }
                self.phase = self.phase.at(Stage::Preparing {
                    since,
                    at: now + rules.limits.retry,
                });
            }
            // Set by `refused`; not preparation: no deadline runs while the
            // TV refuses.
            Ok(Acquired::Refused) => {}
            Err(fault) => return self.recover(fault, Reopen::AfterRetry, now, rules, out),
        }
        self.pace(now, rules, out);
    }
}

impl State {
    /// The driver refused to initialise at `now`: it is kept, created, and
    /// initialised again at the next change of the video signal. The first
    /// refusal of a period starts it and tells the pulls.
    fn refused(&mut self, refusal: Option<Refusal>, now: Instant, out: &mut Vec<Action>) {
        let since_change_ms = self.since_change(now);
        let refusal = if let Some(refusal) = refusal {
            let initializations = refusal.initializations + 1;
            out.push(Action::Note(Note::RefusedAgain {
                initializations,
                since_change_ms,
            }));
            Refusal {
                initializations,
                ..refusal
            }
        } else {
            out.push(Action::Note(Note::Refused { since_change_ms }));
            out.push(Action::Refused(true));
            Refusal {
                since: now,
                initializations: 1,
            }
        };
        self.phase = self.phase.at(Stage::Refused(refusal));
    }

    /// Fatal video faults end the source; others release the tap and
    /// reopen later. UI faults never end the source: lighting idles without
    /// video until the video or its context changes (a fault that may pass)
    /// or the next source (one that cannot).
    fn recover(
        &mut self,
        fault: Fault,
        reopen: Reopen,
        now: Instant,
        rules: &Rules,
        out: &mut Vec<Action>,
    ) {
        let Some(stage) = self.phase.stage() else {
            return;
        };
        let wait = match &mut self.phase {
            Phase::Video { next_fault, .. } => {
                let Some(wait) = video_wait(fault.recovery, reopen, &rules.limits) else {
                    return self.fail(fault.error, out);
                };
                out.push(Action::Note(Note::VideoRetrying {
                    error: fault.error,
                    retry_in: wait,
                    occurrence: std::mem::replace(next_fault, Occurrence::Again),
                }));
                wait
            }
            Phase::Ui(_) => {
                let occurrence = self.ui.occurrence();
                self.ui = UiSupport::after_fault(fault.recovery);
                let error = fault.error;
                out.push(Action::Note(match self.ui {
                    UiSupport::Unsupported => Note::UiUnsupported { error, occurrence },
                    _ => Note::UiFailed { error, occurrence },
                }));
                rules.limits.retry
            }
            Phase::Idle => return,
        };
        if stage.holds_tap() {
            // Reopened `wait` from after the release, which can take until
            // the watchdog.
            out.push(Action::Release);
            self.phase = self.phase.at(Stage::Releasing { wait });
        } else {
            self.phase = self.phase.at(Stage::Closed {
                at: now + wait,
                refusal: stage.refusal(),
            });
            self.pace(now, rules, out);
        }
    }

    /// End the source with `error`; a held tap is released after.
    fn fail(&mut self, error: Error, out: &mut Vec<Action>) {
        out.push(Action::Fail(error));
        if self.phase.stage().is_some_and(Stage::holds_tap) {
            out.push(Action::Release);
        }
        self.phase = Phase::Idle;
    }

    /// Pace on a fixed clock after a step; skip missed ticks instead of
    /// bursting.
    fn pace(&mut self, now: Instant, rules: &Rules, out: &mut Vec<Action>) {
        self.tick += rules.period;
        if now > self.tick {
            self.tick = now;
        }
        out.push(Action::Sleep(self.tick - now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::PixelFormat;

    const LIMITS: Limits = Limits::TV;
    const PERIOD: Duration = Duration::from_millis(100);
    const RULES: Rules = Rules {
        limits: LIMITS,
        include_ui: true,
        period: PERIOD,
    };
    const NO_UI: Rules = Rules {
        include_ui: false,
        ..RULES
    };

    fn ms(t0: Instant, millis: u64) -> Instant {
        t0 + Duration::from_millis(millis)
    }

    fn ready(video: Video, generation: u64, sequence: u64) -> Readiness {
        Readiness {
            video,
            generation,
            sequence,
        }
    }

    fn signal(readiness: Readiness, created_only: bool) -> Event {
        Event::Signal {
            readiness,
            created_only,
        }
    }

    fn acquired(answer: Result<Acquired, Fault>, initializing: bool) -> Event {
        Event::Acquired {
            answer,
            initializing,
        }
    }

    fn frame() -> Frame {
        Frame::fill(1, 4, 2, PixelFormat::RGB24).unwrap()
    }

    fn fault() -> Fault {
        Fault::recoverable(Error::other("interrupted"))
    }

    /// A source at `phase` that saw available video at `t0`, its pace due
    /// at `t0`.
    fn at(t0: Instant, phase: Phase, ui: UiSupport) -> State {
        State {
            phase,
            ui,
            seen: Some(Seen {
                readiness: ready(Video::Available, 0, 1),
                at: t0,
            }),
            tick: t0,
        }
    }

    fn video(stage: Stage) -> Phase {
        Phase::Video {
            stage,
            next_fault: Occurrence::First,
        }
    }

    fn refusal(t0: Instant, initializations: u32) -> Refusal {
        Refusal {
            since: t0,
            initializations,
        }
    }

    /// Runs `event` at `now` and checks the next phase and the actions.
    fn check(
        state: State,
        event: Event,
        now: Instant,
        rules: &Rules,
        phase: Phase,
        actions: &[Action],
    ) -> State {
        let step = next(state, event, now, rules);
        assert_eq!(step.state.phase, phase, "phase after {state:?}");
        assert_eq!(
            format!("{:?}", step.actions),
            format!("{actions:?}"),
            "actions from {state:?}"
        );
        step.state
    }

    fn note(note: Note) -> Action {
        Action::Note(note)
    }

    /// The first signal is a change of video: video opens at once, the TV UI
    /// without video if enabled, else nothing.
    #[test]
    fn the_first_signal_picks_the_tap() {
        let t0 = Instant::now();
        for (video, rules, phase, last) in [
            (
                Video::Available,
                RULES,
                video(Stage::Closed {
                    at: t0,
                    refusal: None,
                }),
                Action::Open(Kind::Video),
            ),
            (
                Video::Unknown,
                NO_UI,
                video(Stage::Closed {
                    at: t0,
                    refusal: None,
                }),
                Action::Open(Kind::Video),
            ),
            (
                Video::Unavailable,
                RULES,
                Phase::Ui(Stage::Closed {
                    at: t0,
                    refusal: None,
                }),
                Action::Open(Kind::Ui),
            ),
            (Video::Unavailable, NO_UI, Phase::Idle, Action::Idle),
        ] {
            let readiness = ready(video, 3, 7);
            let state = check(
                State::new(t0),
                signal(readiness, false),
                t0,
                &rules,
                phase,
                &[
                    note(Note::SignalChanged(readiness)),
                    note(Note::VideoChanged(video)),
                    last,
                ],
            );
            assert_eq!(state.sequence(), Some(7));
        }
    }

    /// An opened tap is acquired in the same step; one reopened in a
    /// refusal goes on initialising.
    #[test]
    fn an_opened_tap_is_acquired_at_once() {
        let t0 = Instant::now();
        for (refusal, stage) in [
            (None, Stage::Running),
            (Some(refusal(t0, 2)), Stage::Reinitialising(refusal(t0, 2))),
        ] {
            let closed = video(Stage::Closed { at: t0, refusal });
            let state = at(t0, closed, UiSupport::Untried);
            check(
                state,
                Event::Opened,
                t0,
                &RULES,
                video(stage),
                &[Action::Acquire],
            );
        }
    }

    /// A video frame is delivered, ends any deadline and makes the next
    /// fault news again; then the pace.
    #[test]
    fn a_frame_is_delivered_and_paced() {
        let t0 = Instant::now();
        for stage in [
            Stage::Running,
            Stage::Preparing { since: t0, at: t0 },
            Stage::Pending { since: t0 },
        ] {
            let phase = Phase::Video {
                stage,
                next_fault: Occurrence::Again,
            };
            check(
                at(t0, phase, UiSupport::Untried),
                acquired(Ok(Acquired::Frame(frame())), false),
                ms(t0, 30),
                &RULES,
                video(Stage::Running),
                &[
                    Action::Deliver(frame()),
                    Action::Sleep(PERIOD - Duration::from_millis(30)),
                ],
            );
        }
    }

    /// The first TV UI frame of a run is reported once.
    #[test]
    fn the_first_ui_frame_is_reported_once() {
        let t0 = Instant::now();
        let frame = frame();
        let reported = note(Note::UiCapturing {
            width: frame.width(),
            height: frame.height(),
        });
        for (ui, report) in [
            (UiSupport::Untried, true),
            (UiSupport::Again, true),
            (UiSupport::Works, false),
        ] {
            let mut actions = Vec::new();
            if report {
                actions.push(reported.clone());
            }
            actions.extend([Action::Deliver(frame.clone()), Action::Sleep(PERIOD)]);
            let state = check(
                at(t0, Phase::Ui(Stage::Running), ui),
                acquired(Ok(Acquired::Frame(frame.clone())), false),
                t0,
                &RULES,
                Phase::Ui(Stage::Running),
                &actions,
            );
            assert_eq!(state.ui, UiSupport::Works);
        }
    }

    /// A pending driver is timed from its first pending answer; one that
    /// stays pending for `pending` is released and reopened at once.
    #[test]
    fn a_pending_driver_stalls_after_its_limit() {
        let t0 = Instant::now();
        let state = check(
            at(t0, video(Stage::Running), UiSupport::Untried),
            acquired(Ok(Acquired::Pending), false),
            t0,
            &RULES,
            video(Stage::Pending { since: t0 }),
            &[Action::Sleep(PERIOD)],
        );
        let state = check(
            state,
            acquired(Ok(Acquired::Pending), false),
            ms(t0, 1999),
            &RULES,
            video(Stage::Pending { since: t0 }),
            &[Action::Sleep(Duration::ZERO)],
        );
        let stalled = Error::other(format!(
            "webOS video produced no frame for {:?}",
            LIMITS.pending
        ));
        let releasing = Phase::Video {
            stage: Stage::Releasing {
                wait: Duration::ZERO,
            },
            next_fault: Occurrence::Again,
        };
        let state = check(
            state,
            acquired(Ok(Acquired::Pending), false),
            ms(t0, 2000),
            &RULES,
            releasing,
            &[
                note(Note::VideoRetrying {
                    error: stalled,
                    retry_in: Duration::ZERO,
                    occurrence: Occurrence::First,
                }),
                Action::Release,
            ],
        );
        // Due again from after the release.
        let released = ms(t0, 7000);
        let closed = Phase::Video {
            stage: Stage::Closed {
                at: released,
                refusal: None,
            },
            next_fault: Occurrence::Again,
        };
        let state = check(
            state,
            Event::Released,
            released,
            &RULES,
            closed,
            &[Action::Sleep(Duration::ZERO)],
        );
        check(
            state,
            Event::Woke,
            released,
            &RULES,
            closed,
            &[Action::Open(Kind::Video)],
        );
    }

    /// A driver not ready is asked again after `retry`, and fails the source
    /// once it stayed not ready for `preparation`, its tap released after.
    #[test]
    fn a_preparing_driver_fails_after_its_limit() {
        let t0 = Instant::now();
        let preparing = |at_ms| {
            video(Stage::Preparing {
                since: t0,
                at: ms(t0, at_ms),
            })
        };
        let state = check(
            at(t0, video(Stage::Running), UiSupport::Untried),
            acquired(Ok(Acquired::NotReady), true),
            t0,
            &RULES,
            preparing(1000),
            &[
                note(Note::InitializationAnswered {
                    answer: Answer::NotReady,
                    since_change_ms: 0,
                }),
                Action::Sleep(PERIOD),
            ],
        );
        // An early wake waits out the rest.
        let state = check(
            state,
            Event::Woke,
            ms(t0, 400),
            &RULES,
            preparing(1000),
            &[Action::Sleep(Duration::from_millis(600))],
        );
        let state = check(
            state,
            acquired(Ok(Acquired::NotReady), false),
            ms(t0, 9999),
            &RULES,
            preparing(10_999),
            &[Action::Sleep(Duration::ZERO)],
        );
        let failed = Error::other(format!(
            "webOS video was not ready within {:?}",
            LIMITS.preparation
        ));
        check(
            state,
            acquired(Ok(Acquired::NotReady), false),
            ms(t0, 10_000),
            &RULES,
            Phase::Idle,
            &[Action::Fail(failed), Action::Release],
        );
    }

    /// Not ready and pending restart each other's wait.
    #[test]
    fn not_ready_and_pending_restart_each_other() {
        let t0 = Instant::now();
        let mut state = at(t0, video(Stage::Running), UiSupport::Untried);
        for (now, answer, stage) in [
            (
                0,
                Acquired::NotReady,
                Stage::Preparing {
                    since: t0,
                    at: ms(t0, 1000),
                },
            ),
            (
                1500,
                Acquired::Pending,
                Stage::Pending {
                    since: ms(t0, 1500),
                },
            ),
            (
                3000,
                Acquired::NotReady,
                Stage::Preparing {
                    since: ms(t0, 3000),
                    at: ms(t0, 4000),
                },
            ),
        ] {
            state = next(state, acquired(Ok(answer), false), ms(t0, now), &RULES).state;
            assert_eq!(state.phase, video(stage), "at {now} ms");
        }
    }

    /// The pace skips missed ticks instead of bursting.
    #[test]
    fn the_pace_skips_missed_ticks() {
        let t0 = Instant::now();
        let late = ms(t0, 950);
        let state = check(
            at(t0, video(Stage::Running), UiSupport::Untried),
            acquired(Ok(Acquired::Frame(frame())), false),
            late,
            &RULES,
            video(Stage::Running),
            &[Action::Deliver(frame()), Action::Sleep(Duration::ZERO)],
        );
        assert_eq!(state.tick, late);
    }

    /// A refusal tells the pulls once and waits for any change of the
    /// signal, which initialises the driver once more; a second refusal
    /// counts on; a not-ready answer then ends it.
    #[test]
    fn a_refusal_waits_for_a_change_then_ends_preparing() {
        let t0 = Instant::now();
        let refused = |n| video(Stage::Refused(refusal(ms(t0, 100), n)));
        let state = check(
            at(t0, video(Stage::Running), UiSupport::Untried),
            acquired(Ok(Acquired::Refused), true),
            ms(t0, 100),
            &RULES,
            refused(1),
            &[
                note(Note::InitializationAnswered {
                    answer: Answer::Refused,
                    since_change_ms: 100,
                }),
                note(Note::Refused {
                    since_change_ms: 100,
                }),
                Action::Refused(true),
                Action::Sleep(Duration::ZERO),
            ],
        );
        let state = check(
            state,
            Event::Woke,
            ms(t0, 200),
            &RULES,
            refused(1),
            &[Action::Idle],
        );
        // Only the sequence changed.
        let same = ready(Video::Available, 0, 2);
        let state = check(
            state,
            signal(same, true),
            ms(t0, 300),
            &RULES,
            video(Stage::Reinitialising(refusal(ms(t0, 100), 1))),
            &[
                note(Note::SignalChanged(same)),
                note(Note::ChangedWhileRefused {
                    video: Video::Available,
                    generation: 0,
                }),
                Action::Acquire,
            ],
        );
        let state = check(
            state,
            acquired(Ok(Acquired::Refused), true),
            ms(t0, 400),
            &RULES,
            refused(2),
            &[
                note(Note::InitializationAnswered {
                    answer: Answer::Refused,
                    since_change_ms: 100,
                }),
                note(Note::RefusedAgain {
                    initializations: 2,
                    since_change_ms: 100,
                }),
                Action::Sleep(Duration::ZERO),
            ],
        );
        let state = next(
            state,
            signal(ready(Video::Available, 0, 3), true),
            ms(t0, 500),
            &RULES,
        )
        .state;
        check(
            state,
            acquired(Ok(Acquired::NotReady), true),
            ms(t0, 600),
            &RULES,
            video(Stage::Preparing {
                since: ms(t0, 600),
                at: ms(t0, 1600),
            }),
            &[
                note(Note::InitializationAnswered {
                    answer: Answer::NotReady,
                    since_change_ms: 100,
                }),
                Action::Refused(false),
                note(Note::RefusalEndedPreparing {
                    initializations: 3,
                    elapsed_ms: 500,
                }),
                Action::Sleep(Duration::ZERO),
            ],
        );
    }

    /// Every answer other than a refusal ends a refusal with its own note,
    /// the acquisition that ended it counted.
    #[test]
    fn every_other_answer_ends_a_refusal() {
        let t0 = Instant::now();
        let now = ms(t0, 250);
        let ended = |answer: &Result<Acquired, Fault>| {
            let (initializations, elapsed_ms) = (2, 250);
            match Answer::of(answer) {
                Answer::NotReady => Note::RefusalEndedPreparing {
                    initializations,
                    elapsed_ms,
                },
                Answer::Accepted => Note::CaptureAllowed {
                    initializations,
                    elapsed_ms,
                },
                _ => Note::RefusalEndedFailure {
                    initializations,
                    elapsed_ms,
                },
            }
        };
        for answer in [
            Ok(Acquired::NotReady),
            Ok(Acquired::Pending),
            Ok(Acquired::Frame(frame())),
            Err(fault()),
        ] {
            let note = ended(&answer);
            let state = at(
                t0,
                video(Stage::Reinitialising(refusal(t0, 1))),
                UiSupport::Untried,
            );
            let step = next(state, acquired(answer, false), now, &RULES);
            assert_eq!(
                format!("{:?}", &step.actions[..2]),
                format!("{:?}", [Action::Refused(false), Action::Note(note)])
            );
            assert_eq!(step.state.phase.stage().and_then(Stage::refusal), None);
        }
    }

    /// A context reset starts a new period: a created-only driver is kept
    /// (and a refusal goes on), any other held tap is released and
    /// reopened, a refusal surviving it.
    #[test]
    fn a_context_reset_keeps_only_a_created_only_driver() {
        let t0 = Instant::now();
        let now = ms(t0, 50);
        let reset = ready(Video::Available, 1, 2);
        let r = refusal(t0, 1);
        let closed = |refusal| video(Stage::Closed { at: now, refusal });
        for (stage, created_only, phase, actions) in [
            (
                Stage::Refused(r),
                true,
                video(Stage::Reinitialising(r)),
                vec![Action::Acquire],
            ),
            (
                Stage::Preparing {
                    since: t0,
                    at: ms(t0, 900),
                },
                true,
                video(Stage::Running),
                vec![Action::Acquire],
            ),
            (
                Stage::Running,
                false,
                closed(None),
                vec![Action::Release, Action::Open(Kind::Video)],
            ),
            (
                Stage::Refused(r),
                false,
                closed(Some(r)),
                vec![Action::Release, Action::Open(Kind::Video)],
            ),
            (
                Stage::Closed {
                    at: ms(t0, 900),
                    refusal: None,
                },
                false,
                closed(None),
                vec![Action::Open(Kind::Video)],
            ),
        ] {
            let mut expected = vec![note(Note::SignalChanged(reset))];
            if stage.refusal().is_some() {
                expected.push(note(Note::ChangedWhileRefused {
                    video: Video::Available,
                    generation: 1,
                }));
            }
            expected.extend(actions);
            let phase_before = Phase::Video {
                stage,
                next_fault: Occurrence::Again,
            };
            check(
                at(t0, phase_before, UiSupport::Untried),
                signal(reset, created_only),
                now,
                &RULES,
                phase,
                &expected,
            );
        }
    }

    /// A refusal that survived a release also survives an open failure.
    #[test]
    fn a_refusal_survives_a_failed_reopen() {
        let t0 = Instant::now();
        let r = Some(refusal(t0, 1));
        let state = at(
            t0,
            video(Stage::Closed { at: t0, refusal: r }),
            UiSupport::Untried,
        );
        check(
            state,
            Event::OpenFailed(fault()),
            ms(t0, 10),
            &RULES,
            Phase::Video {
                stage: Stage::Closed {
                    at: ms(t0, 1010),
                    refusal: r,
                },
                next_fault: Occurrence::Again,
            },
            &[
                note(Note::VideoRetrying {
                    error: Error::other("interrupted"),
                    retry_in: LIMITS.retry,
                    occurrence: Occurrence::First,
                }),
                Action::Sleep(Duration::from_millis(90)),
            ],
        );
    }

    /// Video becoming unavailable releases a refused driver and ends the
    /// refusal.
    #[test]
    fn video_leaving_ends_a_refusal() {
        let t0 = Instant::now();
        let gone = ready(Video::Unavailable, 0, 2);
        let state = at(
            t0,
            video(Stage::Refused(refusal(t0, 3))),
            UiSupport::Untried,
        );
        check(
            state,
            signal(gone, true),
            ms(t0, 700),
            &NO_UI,
            Phase::Idle,
            &[
                note(Note::SignalChanged(gone)),
                note(Note::VideoChanged(Video::Unavailable)),
                note(Note::ChangedWhileRefused {
                    video: Video::Unavailable,
                    generation: 0,
                }),
                Action::Release,
                Action::Refused(false),
                note(Note::RefusalEndedWithVideo {
                    initializations: 3,
                    elapsed_ms: 700,
                }),
                Action::Idle,
            ],
        );
    }

    /// A recoverable fault in any held video stage releases the tap and
    /// reopens `retry` after the release; only the first of a period is a
    /// warning. A fatal one fails the source, its tap released after.
    #[test]
    fn a_fault_in_each_video_stage() {
        let t0 = Instant::now();
        let r = refusal(t0, 1);
        for stage in [
            Stage::Running,
            Stage::Preparing { since: t0, at: t0 },
            Stage::Pending { since: t0 },
            Stage::Reinitialising(r),
        ] {
            for next_fault in [Occurrence::First, Occurrence::Again] {
                let mut expected = Vec::new();
                if stage.refusal().is_some() {
                    expected.push(Action::Refused(false));
                    expected.push(note(Note::RefusalEndedFailure {
                        initializations: 2,
                        elapsed_ms: 0,
                    }));
                }
                let mut fatal = expected.clone();
                expected.push(note(Note::VideoRetrying {
                    error: Error::other("interrupted"),
                    retry_in: LIMITS.retry,
                    occurrence: next_fault,
                }));
                expected.push(Action::Release);
                let state = at(t0, Phase::Video { stage, next_fault }, UiSupport::Untried);
                let releasing = Phase::Video {
                    stage: Stage::Releasing { wait: LIMITS.retry },
                    next_fault: Occurrence::Again,
                };
                check(
                    state,
                    acquired(Err(fault()), false),
                    t0,
                    &RULES,
                    releasing,
                    &expected,
                );
                let error = Error::other("gone");
                let answer = Err(Fault::fatal(error.clone()));
                fatal.extend([Action::Fail(error), Action::Release]);
                check(
                    state,
                    acquired(answer, false),
                    t0,
                    &RULES,
                    Phase::Idle,
                    &fatal,
                );
            }
        }
        // A fatal open fault: nothing to release.
        let closed = video(Stage::Closed {
            at: t0,
            refusal: None,
        });
        let error = Error::other("no library");
        check(
            at(t0, closed, UiSupport::Untried),
            Event::OpenFailed(Fault::fatal(error.clone())),
            t0,
            &RULES,
            Phase::Idle,
            &[Action::Fail(error)],
        );
    }

    /// A stop releases the tap iff one is held.
    #[test]
    fn a_stop_releases_only_a_held_tap() {
        let t0 = Instant::now();
        let r = refusal(t0, 1);
        let closed = Stage::Closed {
            at: t0,
            refusal: Some(r),
        };
        let releasing = Stage::Releasing { wait: LIMITS.retry };
        let held = [
            Stage::Running,
            Stage::Preparing { since: t0, at: t0 },
            Stage::Pending { since: t0 },
            Stage::Refused(r),
            Stage::Reinitialising(r),
        ];
        let mut cases = vec![(Phase::Idle, false)];
        for stage in [closed, releasing] {
            cases.extend([(video(stage), false), (Phase::Ui(stage), false)]);
        }
        for stage in held {
            cases.extend([(video(stage), true), (Phase::Ui(stage), true)]);
        }
        for (phase, release) in cases {
            let actions: &[Action] = if release { &[Action::Release] } else { &[] };
            check(
                at(t0, phase, UiSupport::Works),
                Event::Stop,
                t0,
                &RULES,
                Phase::Idle,
                actions,
            );
        }
    }

    /// A tap being released waits for its release, whatever else comes.
    #[test]
    fn a_releasing_tap_waits_for_its_release() {
        let t0 = Instant::now();
        let releasing = video(Stage::Releasing { wait: LIMITS.retry });
        check(
            at(t0, releasing, UiSupport::Untried),
            Event::Woke,
            t0,
            &RULES,
            releasing,
            &[],
        );
    }

    /// A failed TV UI is left until the video comes back and goes; then it
    /// is tried again, its next fault no news.
    #[test]
    fn a_failed_ui_is_tried_again_after_video_comes_and_goes() {
        let t0 = Instant::now();
        let mut state = at(t0, Phase::Ui(Stage::Running), UiSupport::Works);
        state.seen = Some(Seen {
            readiness: ready(Video::Unavailable, 0, 1),
            at: t0,
        });
        let releasing = Phase::Ui(Stage::Releasing { wait: LIMITS.retry });
        let ui_failed = |occurrence| {
            note(Note::UiFailed {
                error: Error::other("interrupted"),
                occurrence,
            })
        };
        let state = check(
            state,
            acquired(Err(fault()), false),
            t0,
            &RULES,
            releasing,
            &[ui_failed(Occurrence::First), Action::Release],
        );
        assert_eq!(state.ui, UiSupport::Failed);
        let closed = Phase::Ui(Stage::Closed {
            at: ms(t0, 1000),
            refusal: None,
        });
        let state = check(
            state,
            Event::Released,
            t0,
            &RULES,
            closed,
            &[Action::Sleep(PERIOD)],
        );
        // The next run of the loop finds no tap wanted.
        let state = check(
            state,
            Event::Woke,
            ms(t0, 100),
            &RULES,
            Phase::Idle,
            &[Action::Idle],
        );
        let back = ready(Video::Available, 0, 2);
        let state = next(state, signal(back, false), ms(t0, 200), &RULES).state;
        assert_eq!(state.ui, UiSupport::Again);
        let state = next(state, Event::Opened, ms(t0, 200), &RULES).state;
        let gone = ready(Video::Unavailable, 0, 3);
        let state = check(
            state,
            signal(gone, false),
            ms(t0, 300),
            &RULES,
            Phase::Ui(Stage::Closed {
                at: ms(t0, 300),
                refusal: None,
            }),
            &[
                note(Note::SignalChanged(gone)),
                note(Note::VideoChanged(Video::Unavailable)),
                Action::Release,
                Action::Open(Kind::Ui),
            ],
        );
        let state = check(
            state,
            Event::OpenFailed(fault()),
            ms(t0, 300),
            &RULES,
            Phase::Ui(Stage::Closed {
                at: ms(t0, 1300),
                refusal: None,
            }),
            &[ui_failed(Occurrence::Again), Action::Sleep(Duration::ZERO)],
        );
        assert_eq!(state.ui, UiSupport::Failed);
    }

    /// A UI fault's `retry` never elapses: the next run finds no UI wanted,
    /// and a context reset that revives it opens it at once.
    #[test]
    fn a_ui_fault_retry_is_overtaken() {
        let t0 = Instant::now();
        let closed = Phase::Ui(Stage::Closed {
            at: ms(t0, 1000),
            refusal: None,
        });
        let mut state = at(t0, closed, UiSupport::Failed);
        state.seen = Some(Seen {
            readiness: ready(Video::Unavailable, 0, 1),
            at: t0,
        });
        let reset = ready(Video::Unavailable, 1, 2);
        check(
            state,
            signal(reset, false),
            ms(t0, 100),
            &RULES,
            Phase::Ui(Stage::Closed {
                at: ms(t0, 100),
                refusal: None,
            }),
            &[note(Note::SignalChanged(reset)), Action::Open(Kind::Ui)],
        );
        // Only the sequence: the UI stays failed, so no tap is wanted.
        let same = ready(Video::Unavailable, 0, 2);
        check(
            state,
            signal(same, false),
            ms(t0, 100),
            &RULES,
            Phase::Idle,
            &[note(Note::SignalChanged(same)), Action::Idle],
        );
    }

    /// A UI fault that cannot heal is final for the source.
    #[test]
    fn an_unsupported_ui_is_never_opened_again() {
        let t0 = Instant::now();
        let closed = Phase::Ui(Stage::Closed {
            at: t0,
            refusal: None,
        });
        let error = Error::other("no library");
        let state = next(
            at(t0, closed, UiSupport::Untried),
            Event::OpenFailed(Fault::fatal(error.clone())),
            t0,
            &RULES,
        );
        assert_eq!(
            format!("{:?}", step_notes(&state.actions)),
            format!(
                "{:?}",
                [Note::UiUnsupported {
                    error,
                    occurrence: Occurrence::First
                }]
            )
        );
        let mut state = state.state;
        for (sequence, video) in [(2, Video::Available), (3, Video::Unavailable)] {
            state = next(state, signal(ready(video, 1, sequence), false), t0, &RULES).state;
        }
        assert_eq!(state.ui, UiSupport::Unsupported);
        assert_eq!(state.phase, Phase::Idle);
    }

    fn step_notes(actions: &[Action]) -> Vec<Note> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Note(note) => Some(note.clone()),
                _ => None,
            })
            .collect()
    }

    /// Two changes read as one leave only the sequence changed: no video
    /// change, so a held tap is kept, where reading them apart releases and
    /// reopens it. Two sequence-only changes in a row equal one.
    #[test]
    fn changes_read_together_are_one_change() {
        let t0 = Instant::now();
        let running = at(t0, video(Stage::Running), UiSupport::Untried);
        let gone = ready(Video::Unavailable, 0, 2);
        let back = ready(Video::Available, 0, 3);
        let apart = next(running, signal(gone, false), t0, &NO_UI).state;
        check(
            apart,
            signal(back, false),
            t0,
            &NO_UI,
            video(Stage::Closed {
                at: t0,
                refusal: None,
            }),
            &[
                note(Note::SignalChanged(back)),
                note(Note::VideoChanged(Video::Available)),
                Action::Open(Kind::Video),
            ],
        );
        check(
            running,
            signal(back, false),
            t0,
            &NO_UI,
            video(Stage::Running),
            &[note(Note::SignalChanged(back)), Action::Acquire],
        );
        let refused = at(
            t0,
            video(Stage::Refused(refusal(t0, 1))),
            UiSupport::Untried,
        );
        let once = next(
            refused,
            signal(ready(Video::Available, 0, 3), true),
            t0,
            &RULES,
        );
        let first = next(
            refused,
            signal(ready(Video::Available, 0, 2), true),
            t0,
            &RULES,
        );
        let twice = next(
            first.state,
            signal(ready(Video::Available, 0, 3), true),
            t0,
            &RULES,
        );
        assert_eq!(once.state, twice.state);
        assert_eq!(
            format!("{:?}", &once.actions[1..]),
            format!("{:?}", &twice.actions[1..])
        );
    }

    /// Every video fault's recovery meets every reopen: a fatal fault ends
    /// the source, a stall reopens at once, any other after `retry`.
    #[test]
    fn every_video_fault_meets_every_reopen() {
        for (recovery, reopen, wait) in [
            (Recovery::Never, Reopen::AtOnce, None),
            (Recovery::Never, Reopen::AfterRetry, None),
            (Recovery::Reopen, Reopen::AtOnce, Some(Duration::ZERO)),
            (Recovery::Reopen, Reopen::AfterRetry, Some(LIMITS.retry)),
        ] {
            assert_eq!(
                video_wait(recovery, reopen, &LIMITS),
                wait,
                "{recovery:?} x {reopen:?}"
            );
        }
    }

    /// Every UI state meets every event: a fault that cannot heal is final,
    /// one that may pass waits for a change of video or context, and a
    /// fault after a failure is no news.
    #[test]
    fn every_ui_state_meets_every_event() {
        use UiSupport::{Again, Failed, Unsupported, Untried, Works};
        for (state, change, occurrence, may_open) in [
            (Untried, Untried, Occurrence::First, true),
            (Works, Works, Occurrence::First, true),
            (Failed, Again, Occurrence::Again, false),
            (Again, Again, Occurrence::Again, true),
            (Unsupported, Unsupported, Occurrence::Again, false),
        ] {
            assert_eq!(state.after_change(), change, "{state:?}");
            assert_eq!(state.occurrence(), occurrence, "{state:?}");
            assert_eq!(state.may_open(), may_open, "{state:?}");
        }
        assert_eq!(UiSupport::after_fault(Recovery::Never), Unsupported);
        assert_eq!(UiSupport::after_fault(Recovery::Reopen), Failed);
    }

    /// A failed or unsupported UI is not opened; video always is.
    #[test]
    fn the_tap_follows_the_video_the_setting_and_the_ui_state() {
        use UiSupport::{Again, Failed, Unsupported, Untried, Works};
        for ui in [Untried, Works, Failed, Again, Unsupported] {
            for include_ui in [false, true] {
                for video in [Video::Available, Video::Unknown] {
                    assert_eq!(wanted(video, include_ui, ui), Some(Kind::Video));
                }
                let ui_tap = (include_ui && ui.may_open()).then_some(Kind::Ui);
                assert_eq!(
                    wanted(Video::Unavailable, include_ui, ui),
                    ui_tap,
                    "{ui:?}, {include_ui}"
                );
            }
        }
    }

    /// A TV UI tap answers as a video one would: a stall waits `retry`, a
    /// driver not ready too long fails the source.
    #[test]
    fn a_ui_tap_stalls_and_prepares_like_video() {
        let t0 = Instant::now();
        let stalled = Error::other(format!(
            "webOS video produced no frame for {:?}",
            LIMITS.pending
        ));
        let state = at(
            t0,
            Phase::Ui(Stage::Pending { since: t0 }),
            UiSupport::Works,
        );
        let after = check(
            state,
            acquired(Ok(Acquired::Pending), false),
            ms(t0, 2000),
            &RULES,
            Phase::Ui(Stage::Releasing { wait: LIMITS.retry }),
            &[
                note(Note::UiFailed {
                    error: stalled,
                    occurrence: Occurrence::First,
                }),
                Action::Release,
            ],
        );
        assert_eq!(after.ui, UiSupport::Failed);
        let preparing = Phase::Ui(Stage::Preparing { since: t0, at: t0 });
        let failed = Error::other(format!(
            "webOS video was not ready within {:?}",
            LIMITS.preparation
        ));
        check(
            at(t0, preparing, UiSupport::Works),
            acquired(Ok(Acquired::NotReady), false),
            ms(t0, 10_000),
            &RULES,
            Phase::Idle,
            &[Action::Fail(failed), Action::Release],
        );
    }

    /// A change of kind ends a refusal that outlived its tap, with nothing
    /// to release.
    #[test]
    fn a_change_of_kind_ends_a_refusal_without_a_tap() {
        let t0 = Instant::now();
        let closed = video(Stage::Closed {
            at: ms(t0, 500),
            refusal: Some(refusal(t0, 2)),
        });
        let gone = ready(Video::Unavailable, 0, 2);
        check(
            at(t0, closed, UiSupport::Untried),
            signal(gone, false),
            ms(t0, 40),
            &NO_UI,
            Phase::Idle,
            &[
                note(Note::SignalChanged(gone)),
                note(Note::VideoChanged(Video::Unavailable)),
                note(Note::ChangedWhileRefused {
                    video: Video::Unavailable,
                    generation: 0,
                }),
                Action::Refused(false),
                note(Note::RefusalEndedWithVideo {
                    initializations: 2,
                    elapsed_ms: 40,
                }),
                Action::Idle,
            ],
        );
    }

    /// An early wake before a reopen waits out the rest; a second open
    /// failure of a period is no news.
    #[test]
    fn a_closed_tap_waits_and_repeats_quietly() {
        let t0 = Instant::now();
        let closed = Phase::Video {
            stage: Stage::Closed {
                at: ms(t0, 1000),
                refusal: None,
            },
            next_fault: Occurrence::Again,
        };
        let state = at(t0, closed, UiSupport::Untried);
        let state = check(
            state,
            Event::Woke,
            ms(t0, 300),
            &RULES,
            closed,
            &[Action::Sleep(Duration::from_millis(700))],
        );
        let state = next(state, Event::Woke, ms(t0, 1000), &RULES).state;
        let reopened = Phase::Video {
            stage: Stage::Closed {
                at: ms(t0, 2000),
                refusal: None,
            },
            next_fault: Occurrence::Again,
        };
        check(
            state,
            Event::OpenFailed(fault()),
            ms(t0, 1000),
            &RULES,
            reopened,
            &[
                note(Note::VideoRetrying {
                    error: Error::other("interrupted"),
                    retry_in: LIMITS.retry,
                    occurrence: Occurrence::Again,
                }),
                Action::Sleep(Duration::ZERO),
            ],
        );
    }

    /// A context reset reopens a held TV UI tap and leaves an idle source
    /// idle.
    #[test]
    fn a_context_reset_without_video() {
        let t0 = Instant::now();
        let reset = ready(Video::Unavailable, 1, 2);
        for (phase, rules, after, actions) in [
            (
                Phase::Ui(Stage::Running),
                RULES,
                Phase::Ui(Stage::Closed {
                    at: t0,
                    refusal: None,
                }),
                vec![Action::Release, Action::Open(Kind::Ui)],
            ),
            (Phase::Idle, NO_UI, Phase::Idle, vec![Action::Idle]),
        ] {
            let mut state = at(t0, phase, UiSupport::Works);
            state.seen = Some(Seen {
                readiness: ready(Video::Unavailable, 0, 1),
                at: t0,
            });
            let mut expected = vec![note(Note::SignalChanged(reset))];
            expected.extend(actions);
            check(state, signal(reset, false), t0, &rules, after, &expected);
        }
    }
}
