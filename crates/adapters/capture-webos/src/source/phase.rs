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

/// A refusal of the held driver: from an initialisation the driver
/// refused to the first answer that is not a refusal, or a change of tap
/// kind. The pulls are told refused meanwhile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Refusal {
    since: Instant,
    /// Initialisations of the created driver in this refusal, the refused
    /// one that started it included.
    initializations: u32,
}

/// Where a video tap's period stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    /// No tap: open at `at`. While a [`Then::Reopen`] release runs, `at`
    /// counts from its start; [`Event::Released`] counts it again from its
    /// end.
    Closed { at: Instant },
    /// A tap with no deadline running: acquired at the pace.
    Running,
    /// The driver answered not ready since `since`; acquired again at `at`.
    Preparing { since: Instant, at: Instant },
    /// Initialised, with no buffer since `since`.
    Pending { since: Instant },
    /// The driver refused: initialised again at the next change of the
    /// video signal, since nothing tells when the TV would allow it sooner.
    /// A refused driver stays created-only, so a refusal always has its tap.
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
    /// The TV UI tap, `held` once opened. It answers only a frame or a
    /// fault (its driver has no separate initialisation), so it has none of
    /// the video tap's stages: a fault ends its period.
    Ui { held: bool },
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
    /// The release of a [`Then::Reopen`] completed; `wait` is the one it
    /// carried.
    Released {
        wait: Duration,
    },
    /// A sleep or an idle ended without a change of the signal. The wanted
    /// tap changes only with a signal (a TV UI fault ends its period
    /// itself), so only the due step is decided.
    Woke,
    Stop,
}

/// What the shell performs, in order. A failing release fails the source.
#[derive(Debug, Clone)]
pub(super) enum Action {
    Open(Kind),
    Acquire,
    /// Release the held tap, then do as `Then` says.
    Release(Then),
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

/// What follows an [`Action::Release`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Then {
    /// Go on with the step's other actions.
    Nothing,
    /// Answer with [`Event::Released`] and this `wait`: the video tap was
    /// released after a fault and reopens `wait` after the release
    /// completes.
    Reopen { wait: Duration },
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

/// A log line, decided here and written by the shell, which holds its text
/// and level; an [`Occurrence`] picks a warning or debug.
#[derive(Debug, Clone)]
pub(super) enum Note {
    /// The signal has a new sequence.
    SignalChanged(Readiness),
    /// The video's availability changed.
    VideoChanged(Video),
    /// The signal changed during a refusal.
    ChangedWhileRefused { video: Video, generation: u64 },
    /// An initialisation of a created-only driver answered.
    InitializationAnswered {
        answer: Answer,
        since_change_ms: u128,
    },
    /// A refusal starts.
    Refused { since_change_ms: u128 },
    /// A refusal goes on.
    RefusedAgain {
        initializations: u32,
        since_change_ms: u128,
    },
    /// A change of tap kind ended a refusal.
    RefusalEndedWithVideo {
        initializations: u32,
        elapsed_ms: u128,
    },
    /// A not-ready answer ended a refusal.
    RefusalEndedPreparing {
        initializations: u32,
        elapsed_ms: u128,
    },
    /// A frame or a pending buffer ended a refusal.
    CaptureAllowed {
        initializations: u32,
        elapsed_ms: u128,
    },
    /// A native failure ended a refusal.
    RefusalEndedFailure {
        initializations: u32,
        elapsed_ms: u128,
    },
    /// The first TV UI frame of the source or since a UI fault.
    UiCapturing { width: u32, height: u32 },
    /// A TV UI fault that may pass.
    UiFailed {
        error: Error,
        occurrence: Occurrence,
    },
    /// A TV UI fault that cannot heal.
    UiUnsupported {
        error: Error,
        occurrence: Occurrence,
    },
    /// A recoverable video fault: the tap reopens in `retry_in`.
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
    pub interval: Duration,
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
        !matches!(self, Self::Closed { .. })
    }

    fn refusal(self) -> Option<Refusal> {
        match self {
            Self::Refused(refusal) | Self::Reinitialising(refusal) => Some(refusal),
            Self::Closed { .. } | Self::Running | Self::Preparing { .. } | Self::Pending { .. } => {
                None
            }
        }
    }
}

impl Phase {
    /// A new period for `kind` at `stage`: its first video fault is news. A
    /// TV UI period keeps only whether `stage` holds a tap.
    fn begin(kind: Option<Kind>, stage: Stage) -> Self {
        match kind {
            None => Self::Idle,
            Some(Kind::Video) => Self::Video {
                stage,
                next_fault: Occurrence::First,
            },
            Some(Kind::Ui) => Self::Ui {
                held: stage.holds_tap(),
            },
        }
    }

    fn kind(self) -> Option<Kind> {
        match self {
            Self::Idle => None,
            Self::Video { .. } => Some(Kind::Video),
            Self::Ui { .. } => Some(Kind::Ui),
        }
    }

    fn holds_tap(self) -> bool {
        match self {
            Self::Idle => false,
            Self::Video { stage, .. } => stage.holds_tap(),
            Self::Ui { held } => held,
        }
    }

    /// The video tap's refusal; none for any other.
    fn refusal(self) -> Option<Refusal> {
        match self {
            Self::Video { stage, .. } => stage.refusal(),
            Self::Idle | Self::Ui { .. } => None,
        }
    }

    /// The same video period at `stage`.
    fn at(self, stage: Stage) -> Self {
        match self {
            Self::Video { next_fault, .. } => Self::Video { stage, next_fault },
            Self::Idle | Self::Ui { .. } => unreachable!("only a video period has stages"),
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
            state.choose_tap(context_changed, created_only, now, rules, out);
            state.decide(now, out);
        }
        Event::Woke => state.decide(now, out),
        Event::Opened => state.opened(out),
        Event::OpenFailed(fault) => state.recover(fault, Reopen::AfterRetry, now, rules, out),
        Event::Acquired {
            answer,
            initializing,
        } => state.acquired(answer, initializing, now, rules, out),
        Event::Released { wait } => {
            state.phase = state.phase.at(Stage::Closed { at: now + wait });
            state.pace(now, rules, out);
        }
        Event::Stop => {
            if state.phase.holds_tap() {
                out.push(Action::Release(Then::Nothing));
            }
            state.phase = Phase::Idle;
        }
    }
    Step { state, actions }
}

/// An acquisition at `now` that is not a refusal ends any `refusal`: the
/// pulls are told, and `note` is given the initialisations counted with that
/// acquisition and the milliseconds since the refusal started.
fn end_refusal(
    refusal: Option<Refusal>,
    now: Instant,
    out: &mut Vec<Action>,
    note: impl FnOnce(u32, u128) -> Note,
) {
    if let Some(refusal) = refusal {
        out.push(Action::Refused(false));
        out.push(Action::Note(note(
            refusal.initializations + 1,
            millis(refusal.since, now),
        )));
    }
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
        if self.phase.refusal().is_some() {
            if let Phase::Video {
                stage: Stage::Refused(refusal),
                ..
            } = self.phase
            {
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
    /// it; a change of kind also ends a refusal. A refused driver stays
    /// created-only, so a refusal's driver is always kept otherwise.
    fn choose_tap(
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
        let held = self.phase.holds_tap();
        let refusal = self.phase.refusal();
        let stage = if kind != self.phase.kind() {
            if held {
                out.push(Action::Release(Then::Nothing));
            }
            if let Some(refusal) = refusal {
                out.push(Action::Refused(false));
                out.push(Action::Note(Note::RefusalEndedWithVideo {
                    initializations: refusal.initializations,
                    elapsed_ms: millis(refusal.since, now),
                }));
            }
            Stage::Closed { at: now }
        } else if let Some(refusal) = refusal {
            debug_assert!(created_only, "a refused driver stays created-only");
            Stage::Reinitialising(refusal)
        } else if held && created_only {
            Stage::Running
        } else {
            if held {
                out.push(Action::Release(Then::Nothing));
            }
            Stage::Closed { at: now }
        };
        self.phase = Phase::begin(kind, stage);
    }

    /// Idle without a tap wanted or while refused; wait for a deadline not
    /// reached; else open or acquire.
    fn decide(&self, now: Instant, out: &mut Vec<Action>) {
        out.push(match self.phase {
            Phase::Idle
            | Phase::Video {
                stage: Stage::Refused(_),
                ..
            } => Action::Idle,
            Phase::Ui { held: false } => Action::Open(Kind::Ui),
            Phase::Ui { held: true } => Action::Acquire,
            Phase::Video {
                stage: Stage::Closed { at } | Stage::Preparing { at, .. },
                ..
            } if now < at => Action::Sleep(at - now),
            Phase::Video {
                stage: Stage::Closed { .. },
                ..
            } => Action::Open(Kind::Video),
            Phase::Video { .. } => Action::Acquire,
        });
    }

    /// The tap opened: acquire at once, in the same step.
    fn opened(&mut self, out: &mut Vec<Action>) {
        self.phase = match self.phase {
            Phase::Video {
                stage: Stage::Closed { .. },
                ..
            } => self.phase.at(Stage::Running),
            Phase::Ui { held: false } => Phase::Ui { held: true },
            _ => return,
        };
        out.push(Action::Acquire);
    }
}

impl State {
    /// Act on one acquisition; a TV UI answer goes to [`Self::ui_acquired`].
    /// For video, an `initializing` one if the driver was created-only: a
    /// refusal starts, goes on or ends; then publish a frame, time a pending
    /// or preparing driver, or recover from a fault.
    fn acquired(
        &mut self,
        answer: Result<Acquired, Fault>,
        initializing: bool,
        now: Instant,
        rules: &Rules,
        out: &mut Vec<Action>,
    ) {
        let stage = match self.phase {
            Phase::Video { stage, .. } => stage,
            Phase::Ui { .. } => return self.ui_acquired(answer, now, rules, out),
            Phase::Idle => {
                unreachable!("an acquisition answers an acquire, asked only of a tap's period")
            }
        };
        let outcome = Answer::of(&answer);
        if initializing {
            out.push(Action::Note(Note::InitializationAnswered {
                answer: outcome,
                since_change_ms: self.since_change(now),
            }));
        }
        let refusal = stage.refusal();
        match outcome {
            Answer::Refused => self.refused(refusal, now, out),
            Answer::NotReady => end_refusal(refusal, now, out, |initializations, elapsed_ms| {
                Note::RefusalEndedPreparing {
                    initializations,
                    elapsed_ms,
                }
            }),
            Answer::Accepted => end_refusal(refusal, now, out, |initializations, elapsed_ms| {
                Note::CaptureAllowed {
                    initializations,
                    elapsed_ms,
                }
            }),
            Answer::Failed => end_refusal(refusal, now, out, |initializations, elapsed_ms| {
                Note::RefusalEndedFailure {
                    initializations,
                    elapsed_ms,
                }
            }),
        }
        match answer {
            Ok(Acquired::Frame(frame)) => {
                self.phase = Phase::Video {
                    stage: Stage::Running,
                    next_fault: Occurrence::First,
                };
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

    /// A TV UI tap answers only a frame or a fault, as its tap
    /// ([`super::Tap`] over the HAL GAL plane) is written: the first frame
    /// since a fault is reported once, delivered and paced; a fault ends
    /// the UI period.
    fn ui_acquired(
        &mut self,
        answer: Result<Acquired, Fault>,
        now: Instant,
        rules: &Rules,
        out: &mut Vec<Action>,
    ) {
        match answer {
            Ok(Acquired::Frame(frame)) => {
                if self.ui != UiSupport::Works {
                    self.ui = UiSupport::Works;
                    out.push(Action::Note(Note::UiCapturing {
                        width: frame.width(),
                        height: frame.height(),
                    }));
                }
                out.push(Action::Deliver(frame));
                self.pace(now, rules, out);
            }
            Err(fault) => self.ui_fault(fault, now, out),
            Ok(other) => unreachable!("a TV UI tap answers only a frame or a fault, not {other:?}"),
        }
    }
}

impl State {
    /// The driver refused to initialise at `now`: it is kept, created, and
    /// initialised again at the next change of the video signal. A refusal
    /// with none running starts one and tells the pulls.
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

    /// A fault of the period's open or acquire: a TV UI fault or a video
    /// one.
    fn recover(
        &mut self,
        fault: Fault,
        reopen: Reopen,
        now: Instant,
        rules: &Rules,
        out: &mut Vec<Action>,
    ) {
        match self.phase {
            Phase::Video { .. } => self.video_fault(fault, reopen, now, rules, out),
            Phase::Ui { .. } => self.ui_fault(fault, now, out),
            Phase::Idle => {
                unreachable!("a fault answers an open or acquire, asked only of a tap's period")
            }
        }
    }

    /// A TV UI fault never ends the source, and ends the UI period: a held
    /// tap is released and lighting idles without video until the video or
    /// its context changes (a fault that may pass) or the next source (one
    /// that cannot).
    fn ui_fault(&mut self, fault: Fault, now: Instant, out: &mut Vec<Action>) {
        let occurrence = self.ui.occurrence();
        self.ui = UiSupport::after_fault(fault.recovery);
        let error = fault.error;
        out.push(Action::Note(match self.ui {
            UiSupport::Unsupported => Note::UiUnsupported { error, occurrence },
            _ => Note::UiFailed { error, occurrence },
        }));
        if self.phase.holds_tap() {
            out.push(Action::Release(Then::Nothing));
        }
        self.phase = Phase::Idle;
        self.decide(now, out);
    }

    /// A fatal video fault ends the source; any other releases the tap and
    /// reopens it later.
    fn video_fault(
        &mut self,
        fault: Fault,
        reopen: Reopen,
        now: Instant,
        rules: &Rules,
        out: &mut Vec<Action>,
    ) {
        let Phase::Video { stage, next_fault } = &mut self.phase else {
            unreachable!("a video fault answers a video period");
        };
        let stage = *stage;
        let Some(wait) = video_wait(fault.recovery, reopen, &rules.limits) else {
            return self.fail(fault.error, out);
        };
        out.push(Action::Note(Note::VideoRetrying {
            error: fault.error,
            retry_in: wait,
            occurrence: std::mem::replace(next_fault, Occurrence::Again),
        }));
        self.phase = self.phase.at(Stage::Closed { at: now + wait });
        if stage.holds_tap() {
            // Reopened `wait` from after the release, which can take until
            // the watchdog: [`Event::Released`] counts it again and paces.
            out.push(Action::Release(Then::Reopen { wait }));
        } else {
            self.pace(now, rules, out);
        }
    }

    /// End the source with `error`; a held tap is released after.
    fn fail(&mut self, error: Error, out: &mut Vec<Action>) {
        out.push(Action::Fail(error));
        if self.phase.holds_tap() {
            out.push(Action::Release(Then::Nothing));
        }
        self.phase = Phase::Idle;
    }

    /// Pace on a fixed clock after a step; skip missed ticks instead of
    /// bursting.
    fn pace(&mut self, now: Instant, rules: &Rules, out: &mut Vec<Action>) {
        self.tick += rules.interval;
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
    const INTERVAL: Duration = Duration::from_millis(100);
    const RULES: Rules = Rules {
        limits: LIMITS,
        include_ui: true,
        interval: INTERVAL,
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
                video(Stage::Closed { at: t0 }),
                Action::Open(Kind::Video),
            ),
            (
                Video::Unknown,
                NO_UI,
                video(Stage::Closed { at: t0 }),
                Action::Open(Kind::Video),
            ),
            (
                Video::Unavailable,
                RULES,
                Phase::Ui { held: false },
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

    /// An opened tap is acquired in the same step.
    #[test]
    fn an_opened_tap_is_acquired_at_once() {
        let t0 = Instant::now();
        let state = at(t0, video(Stage::Closed { at: t0 }), UiSupport::Untried);
        check(
            state,
            Event::Opened,
            t0,
            &RULES,
            video(Stage::Running),
            &[Action::Acquire],
        );
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
                    Action::Sleep(INTERVAL - Duration::from_millis(30)),
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
            actions.extend([Action::Deliver(frame.clone()), Action::Sleep(INTERVAL)]);
            let state = check(
                at(t0, Phase::Ui { held: true }, ui),
                acquired(Ok(Acquired::Frame(frame.clone())), false),
                t0,
                &RULES,
                Phase::Ui { held: true },
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
            &[Action::Sleep(INTERVAL)],
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
        let reopening = Phase::Video {
            stage: Stage::Closed { at: ms(t0, 2000) },
            next_fault: Occurrence::Again,
        };
        let state = check(
            state,
            acquired(Ok(Acquired::Pending), false),
            ms(t0, 2000),
            &RULES,
            reopening,
            &[
                note(Note::VideoRetrying {
                    error: stalled,
                    retry_in: Duration::ZERO,
                    occurrence: Occurrence::First,
                }),
                Action::Release(Then::Reopen {
                    wait: Duration::ZERO,
                }),
            ],
        );
        // Due again from after the release.
        let released = ms(t0, 7000);
        let closed = Phase::Video {
            stage: Stage::Closed { at: released },
            next_fault: Occurrence::Again,
        };
        let state = check(
            state,
            Event::Released {
                wait: Duration::ZERO,
            },
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
                Action::Sleep(INTERVAL),
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
            &[Action::Fail(failed), Action::Release(Then::Nothing)],
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
                Answer::Failed | Answer::Refused => Note::RefusalEndedFailure {
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
            assert_eq!(step.state.phase.refusal(), None);
        }
    }

    /// A context reset starts a new period: a created-only driver is kept
    /// (and a refusal goes on), any other held tap is released and
    /// reopened.
    #[test]
    fn a_context_reset_keeps_only_a_created_only_driver() {
        let t0 = Instant::now();
        let now = ms(t0, 50);
        let reset = ready(Video::Available, 1, 2);
        let r = refusal(t0, 1);
        let closed = video(Stage::Closed { at: now });
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
                closed,
                vec![Action::Release(Then::Nothing), Action::Open(Kind::Video)],
            ),
            (
                Stage::Closed { at: ms(t0, 900) },
                false,
                closed,
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
                Action::Release(Then::Nothing),
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
                expected.push(Action::Release(Then::Reopen { wait: LIMITS.retry }));
                let state = at(t0, Phase::Video { stage, next_fault }, UiSupport::Untried);
                let reopening = Phase::Video {
                    stage: Stage::Closed {
                        at: t0 + LIMITS.retry,
                    },
                    next_fault: Occurrence::Again,
                };
                check(
                    state,
                    acquired(Err(fault()), false),
                    t0,
                    &RULES,
                    reopening,
                    &expected,
                );
                let error = Error::other("gone");
                let answer = Err(Fault::fatal(error.clone()));
                fatal.extend([Action::Fail(error), Action::Release(Then::Nothing)]);
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
        let closed = video(Stage::Closed { at: t0 });
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
        let closed = Stage::Closed { at: t0 };
        let held = [
            Stage::Running,
            Stage::Preparing { since: t0, at: t0 },
            Stage::Pending { since: t0 },
            Stage::Refused(r),
            Stage::Reinitialising(r),
        ];
        let mut cases = vec![
            (Phase::Idle, false),
            (video(closed), false),
            (Phase::Ui { held: false }, false),
            (Phase::Ui { held: true }, true),
        ];
        cases.extend(held.map(|stage| (video(stage), true)));
        for (phase, release) in cases {
            let actions: &[Action] = if release {
                &[Action::Release(Then::Nothing)]
            } else {
                &[]
            };
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

    /// A failed TV UI is left until the video comes back and goes; then it
    /// is tried again, its next fault no news.
    #[test]
    fn a_failed_ui_is_tried_again_after_video_comes_and_goes() {
        let t0 = Instant::now();
        let mut state = at(t0, Phase::Ui { held: true }, UiSupport::Works);
        state.seen = Some(Seen {
            readiness: ready(Video::Unavailable, 0, 1),
            at: t0,
        });
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
            Phase::Idle,
            &[
                ui_failed(Occurrence::First),
                Action::Release(Then::Nothing),
                Action::Idle,
            ],
        );
        assert_eq!(state.ui, UiSupport::Failed);
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
            Phase::Ui { held: false },
            &[
                note(Note::SignalChanged(gone)),
                note(Note::VideoChanged(Video::Unavailable)),
                Action::Release(Then::Nothing),
                Action::Open(Kind::Ui),
            ],
        );
        let state = check(
            state,
            Event::OpenFailed(fault()),
            ms(t0, 300),
            &RULES,
            Phase::Idle,
            &[ui_failed(Occurrence::Again), Action::Idle],
        );
        assert_eq!(state.ui, UiSupport::Failed);
    }

    /// A UI fault ends the UI period with no timer: a wake without a
    /// signal only idles, and a context reset that revives the UI opens it
    /// at once.
    #[test]
    fn a_failed_ui_waits_for_a_signal() {
        let t0 = Instant::now();
        let mut state = at(t0, Phase::Idle, UiSupport::Failed);
        state.seen = Some(Seen {
            readiness: ready(Video::Unavailable, 0, 1),
            at: t0,
        });
        let state = check(
            state,
            Event::Woke,
            ms(t0, 100),
            &RULES,
            Phase::Idle,
            &[Action::Idle],
        );
        let reset = ready(Video::Unavailable, 1, 2);
        check(
            state,
            signal(reset, false),
            ms(t0, 100),
            &RULES,
            Phase::Ui { held: false },
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
        let error = Error::other("no library");
        let state = next(
            at(t0, Phase::Ui { held: false }, UiSupport::Untried),
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
            video(Stage::Closed { at: t0 }),
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

    /// A TV UI fault, of an open or of an acquisition, ends the UI period at
    /// once: a held tap is released, then the source idles. It never fails
    /// the source, even when it cannot heal.
    #[test]
    fn a_ui_fault_goes_straight_to_no_tap() {
        let t0 = Instant::now();
        for (held, fault, ui) in [
            (false, fault(), UiSupport::Failed),
            (true, fault(), UiSupport::Failed),
            (
                false,
                Fault::fatal(Error::other("x")),
                UiSupport::Unsupported,
            ),
            (
                true,
                Fault::fatal(Error::other("x")),
                UiSupport::Unsupported,
            ),
        ] {
            let error = fault.error.clone();
            let event = if held {
                acquired(Err(fault), false)
            } else {
                Event::OpenFailed(fault)
            };
            let occurrence = Occurrence::First;
            let mut actions = vec![note(match ui {
                UiSupport::Unsupported => Note::UiUnsupported { error, occurrence },
                _ => Note::UiFailed { error, occurrence },
            })];
            if held {
                actions.push(Action::Release(Then::Nothing));
            }
            actions.push(Action::Idle);
            let state = at(t0, Phase::Ui { held }, UiSupport::Works);
            let after = check(state, event, t0, &RULES, Phase::Idle, &actions);
            assert_eq!(after.ui, ui, "{held}");
        }
    }

    /// A TV UI tap answers only a frame or a fault; any other answer is a
    /// broken tap.
    #[test]
    fn a_ui_tap_answers_only_a_frame_or_a_fault() {
        for answer in [Acquired::Pending, Acquired::NotReady, Acquired::Refused] {
            let t0 = Instant::now();
            let state = at(t0, Phase::Ui { held: true }, UiSupport::Works);
            let broken =
                std::panic::catch_unwind(|| next(state, acquired(Ok(answer), false), t0, &RULES))
                    .expect_err("a UI tap's non-frame answer panics");
            let message = broken
                .downcast_ref::<String>()
                .expect("a formatted panic message");
            assert!(
                message.contains("a TV UI tap answers only a frame or a fault"),
                "{message}"
            );
        }
    }

    /// An early wake before a reopen waits out the rest; a second open
    /// failure of a period is no news.
    #[test]
    fn a_closed_tap_waits_and_repeats_quietly() {
        let t0 = Instant::now();
        let closed = Phase::Video {
            stage: Stage::Closed { at: ms(t0, 1000) },
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
            stage: Stage::Closed { at: ms(t0, 2000) },
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
                Phase::Ui { held: true },
                RULES,
                Phase::Ui { held: false },
                vec![Action::Release(Then::Nothing), Action::Open(Kind::Ui)],
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
