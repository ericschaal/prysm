//! The native thread's decisions as a pure state machine: an event and the
//! current time in, the next state and the actions to perform out. No clock,
//! no logging, no backend, no worker; every rule is table-tested.
use super::{Acquired, Fault, Limits, UiFault};
use crate::classify::{self, GiveUp, Reason, Recovery, UiRecovery, VideoCause};
use crate::{Readiness, Video};
use prysm_core::Frame;
use prysm_ports::{Attempts, Error, Schedule};
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

/// Whether a TV UI fault, or a refusal or withholding of video, is the
/// first since the last frame of its tap (news) or a repeat (debug).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Occurrence {
    First,
    Again,
}

/// Where a video tap's period stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    /// No tap: opened at once.
    Closed,
    /// A tap with no deadline running: acquired at the pace.
    Running,
    /// No frame since `since`, the first answer without one (not ready or a
    /// pending buffer) since the tap opened or last delivered a frame: the
    /// preparation limit counts from it. `pending_since` is the first of
    /// the pending answers since the driver last answered anything else
    /// (it was initialised then), `None` after a not-ready one: the stall
    /// counts from it. Acquired again at `at`: [`Limits::not_ready_retry`]
    /// after a not-ready answer, at the pace after a pending one.
    Waiting {
        since: Instant,
        pending_since: Option<Instant>,
        at: Instant,
    },
    /// The driver refused its last `initializations` in a row: initialised
    /// again at the next change of the video signal, since nothing tells
    /// when the TV would allow it sooner. A refused driver stays
    /// created-only, so this stage always has its tap.
    Refused { initializations: u32 },
    /// The video signal changed since the refusal: initialised once now.
    Reinitialising { initializations: u32 },
    /// No tap: the TV withholds this video ([`Recovery::Wait`]). Opened
    /// again at the next change of the video signal, since nothing tells
    /// when the TV would give it sooner.
    Withheld,
}

/// The tap the native thread's current period is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    /// No tap wanted: until the video signal changes.
    Idle,
    Video {
        stage: Stage,
        /// The one new driver a fault opens ([`Reason::FreshDriver`],
        /// [`Reason::StalledDriver`]); a frame or a change of the video
        /// signal gives it back.
        fresh: Attempts,
        /// Since when the pulls are told refused: from the period's first
        /// refusal or withholding until a frame, the end of the period (a
        /// change of tap kind) or the source's failure. Nothing else ends
        /// it, and it is the only source of [`Action::Refused`].
        refused_since: Option<Instant>,
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
    UiOpenFailed(UiFault),
    /// A video acquisition answered; `initializing`: the driver was
    /// created-only before it, as the tap said.
    Acquired {
        answer: Result<Acquired, Fault>,
        initializing: bool,
    },
    /// A TV UI acquisition answered.
    UiAcquired(Result<Frame, UiFault>),
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

/// A log line, decided here and written by the shell, which holds its text
/// and level; an [`Occurrence`] picks news or debug.
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
    /// The driver refused an initialisation, its `initializations`-th in
    /// a row; the first refusal or withholding since a frame is news.
    Refused {
        initializations: u32,
        since_change_ms: u128,
        occurrence: Occurrence,
    },
    /// A frame ended the pulls' refused state of `elapsed_ms`.
    CaptureAllowed { elapsed_ms: u128 },
    /// A change of tap kind ended the pulls' refused state of `elapsed_ms`.
    RefusalEndedWithVideo { elapsed_ms: u128 },
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
    /// A video fault: a new driver is opened once.
    VideoRetrying { error: Error },
    /// The TV withholds this video; the first refusal or withholding since
    /// a frame is news.
    Withheld {
        error: Error,
        occurrence: Occurrence,
    },
    /// The signal changed while the TV withholds the video: a new driver
    /// is opened.
    ChangedWhileWithheld { video: Video, generation: u64 },
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
    fn after_fault(recovery: UiRecovery) -> Self {
        match recovery {
            UiRecovery::Unsupported => Self::Unsupported,
            UiRecovery::MayPass => Self::Failed,
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
        match self {
            Self::Closed | Self::Withheld => false,
            Self::Running
            | Self::Waiting { .. }
            | Self::Refused { .. }
            | Self::Reinitialising { .. } => true,
        }
    }

    /// The refusals in a row of a refused driver; none in any other stage.
    fn refusals(self) -> Option<u32> {
        match self {
            Self::Refused { initializations } | Self::Reinitialising { initializations } => {
                Some(initializations)
            }
            Self::Closed | Self::Running | Self::Waiting { .. } | Self::Withheld => None,
        }
    }

    /// When the wait for a frame started: kept while waiting, else `now`.
    fn waiting_since(self, now: Instant) -> Instant {
        match self {
            Self::Waiting { since, .. } => since,
            _ => now,
        }
    }

    /// When the run of pending answers started: kept while it runs, else
    /// `now`.
    fn pending_since(self, now: Instant) -> Instant {
        match self {
            Self::Waiting {
                pending_since: Some(since),
                ..
            } => since,
            _ => now,
        }
    }
}

impl Phase {
    /// A new period for `kind` at `stage`: a video period has its new
    /// driver to open and the pulls not told refused. A TV UI period keeps
    /// only whether `stage` holds a tap.
    fn begin(kind: Option<Kind>, stage: Stage, limits: &Limits) -> Self {
        match kind {
            None => Self::Idle,
            // The two reasons that open a new driver share its one try.
            Some(Kind::Video) => Self::Video {
                stage,
                fresh: Attempts::new(Reason::FreshDriver.schedule(limits)),
                refused_since: None,
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

    /// The video tap's refusals in a row; none for any other.
    fn refusals(self) -> Option<u32> {
        match self {
            Self::Video { stage, .. } => stage.refusals(),
            Self::Idle | Self::Ui { .. } => None,
        }
    }

    /// The same video period at `stage`.
    fn at(self, stage: Stage) -> Self {
        match self {
            Self::Video {
                fresh,
                refused_since,
                ..
            } => Self::Video {
                stage,
                fresh,
                refused_since,
            },
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
        Event::OpenFailed(fault) => state.video_fault(fault, now, out),
        Event::UiOpenFailed(fault) => state.ui_fault(fault, now, out),
        Event::Acquired {
            answer,
            initializing,
        } => state.acquired(answer, initializing, now, rules, out),
        Event::UiAcquired(answer) => state.ui_acquired(answer, now, rules, out),
        Event::Stop => {
            if state.phase.holds_tap() {
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

    /// The video period's pulls are told refused from `now`, unless they
    /// were already: the first refusal or withholding since a frame is
    /// news.
    fn refuse(&mut self, now: Instant, out: &mut Vec<Action>) -> Occurrence {
        let Phase::Video { refused_since, .. } = &mut self.phase else {
            unreachable!("only video is refused");
        };
        if refused_since.is_some() {
            return Occurrence::Again;
        }
        *refused_since = Some(now);
        out.push(Action::Refused(true));
        Occurrence::First
    }

    /// The pulls stop being told refused, if they were: returns since when
    /// they were.
    fn allow(&mut self, out: &mut Vec<Action>) -> Option<Instant> {
        let Phase::Video { refused_since, .. } = &mut self.phase else {
            return None;
        };
        let since = refused_since.take()?;
        out.push(Action::Refused(false));
        Some(since)
    }

    /// The signal changed to `readiness` (the first counts as a change of
    /// video). Any change lets a refused driver be initialised once more
    /// and opens a new driver for withheld video; a change is new
    /// evidence, so it gives the one new driver back. Only a change of
    /// video or context lets a failed TV UI be tried again. Returns whether
    /// the context changed.
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
        if let Phase::Video { fresh, .. } = &mut self.phase {
            fresh.succeeded();
        }
        if self.phase.refusals().is_some() {
            if let Phase::Video {
                stage: Stage::Refused { initializations },
                ..
            } = self.phase
            {
                self.phase = self.phase.at(Stage::Reinitialising { initializations });
            }
            out.push(Action::Note(Note::ChangedWhileRefused {
                video,
                generation,
            }));
        }
        if let Phase::Video {
            stage: Stage::Withheld,
            ..
        } = self.phase
        {
            self.phase = self.phase.at(Stage::Closed);
            out.push(Action::Note(Note::ChangedWhileWithheld {
                video,
                generation,
            }));
        }
        context_changed
    }

    /// A change of the wanted tap kind starts a new period, a change of the
    /// video context a new stage of the same one (its refused state kept),
    /// due at once. A created-only driver
    /// holds no video context and its release can hang, so only a change of
    /// kind releases it; a change of kind also ends the refused state. A
    /// refused driver stays created-only, so it is always kept otherwise.
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
        let refusals = self.phase.refusals();
        if kind != self.phase.kind() {
            if held {
                out.push(Action::Release);
            }
            if let Some(since) = self.allow(out) {
                out.push(Action::Note(Note::RefusalEndedWithVideo {
                    elapsed_ms: millis(since, now),
                }));
            }
            self.phase = Phase::begin(kind, Stage::Closed, &rules.limits);
            return;
        }
        let stage = if let Some(initializations) = refusals {
            debug_assert!(created_only, "a refused driver stays created-only");
            Stage::Reinitialising { initializations }
        } else if held && created_only {
            Stage::Running
        } else {
            if held {
                out.push(Action::Release);
            }
            Stage::Closed
        };
        self.phase = match self.phase {
            Phase::Video { .. } => self.phase.at(stage),
            Phase::Idle | Phase::Ui { .. } => Phase::begin(kind, stage, &rules.limits),
        };
    }

    /// Idle without a tap wanted, while refused or withheld; wait for a
    /// deadline not reached; else open or acquire.
    fn decide(&self, now: Instant, out: &mut Vec<Action>) {
        out.push(match self.phase {
            Phase::Idle
            | Phase::Video {
                stage: Stage::Refused { .. } | Stage::Withheld,
                ..
            } => Action::Idle,
            Phase::Ui { held: false } => Action::Open(Kind::Ui),
            Phase::Ui { held: true } => Action::Acquire,
            Phase::Video {
                stage: Stage::Waiting { at, .. },
                ..
            } if now < at => Action::Sleep(at - now),
            Phase::Video {
                stage: Stage::Closed,
                ..
            } => Action::Open(Kind::Video),
            Phase::Video { .. } => Action::Acquire,
        });
    }

    /// The tap opened: acquire at once, in the same step.
    fn opened(&mut self, out: &mut Vec<Action>) {
        self.phase = match self.phase {
            Phase::Video {
                stage: Stage::Closed,
                ..
            } => self.phase.at(Stage::Running),
            Phase::Ui { held: false } => Phase::Ui { held: true },
            _ => return,
        };
        out.push(Action::Acquire);
    }
}

impl State {
    /// Act on one video acquisition, an `initializing` one if the driver
    /// was created-only: publish a frame, time the wait for one, keep a
    /// refused driver, or recover from a fault.
    fn acquired(
        &mut self,
        answer: Result<Acquired, Fault>,
        initializing: bool,
        now: Instant,
        rules: &Rules,
        out: &mut Vec<Action>,
    ) {
        let Phase::Video { stage, .. } = self.phase else {
            unreachable!("a video acquisition answers an acquire of a video period")
        };
        let outcome = Answer::of(&answer);
        if initializing {
            out.push(Action::Note(Note::InitializationAnswered {
                answer: outcome,
                since_change_ms: self.since_change(now),
            }));
        }
        match answer {
            Ok(Acquired::Frame(frame)) => {
                let Phase::Video { fresh, .. } = &mut self.phase else {
                    unreachable!("checked above");
                };
                fresh.succeeded();
                if let Some(since) = self.allow(out) {
                    out.push(Action::Note(Note::CaptureAllowed {
                        elapsed_ms: millis(since, now),
                    }));
                }
                self.phase = self.phase.at(Stage::Running);
                out.push(Action::Deliver(frame));
            }
            // Stalled once pending for `pending` since the driver was
            // initialised.
            Ok(Acquired::Pending) => {
                let pending_since = stage.pending_since(now);
                if now.duration_since(pending_since) >= rules.limits.pending {
                    let recovery = classify::video(VideoCause::Stall);
                    let stalled = Fault {
                        error: Error::other_escalating(
                            format!(
                                "webOS video produced no frame for {:?}",
                                rules.limits.pending
                            ),
                            recovery.escalation(),
                        ),
                        recovery,
                    };
                    return self.video_fault(stalled, now, out);
                }
                self.phase = self.phase.at(Stage::Waiting {
                    since: stage.waiting_since(now),
                    pending_since: Some(pending_since),
                    at: now,
                });
            }
            // Timed on the whole wait for a frame, which a pending answer
            // may have started, so alternating answers cannot postpone it:
            // not on an `Attempts`, whose clock starts at its own first
            // failure.
            Ok(Acquired::NotReady) => {
                let Schedule::Within { every, limit } =
                    Reason::DriverPreparing.schedule(&rules.limits)
                else {
                    unreachable!("a preparing driver is retried within a limit")
                };
                let since = stage.waiting_since(now);
                if now.duration_since(since) >= limit {
                    let error = Error::other(format!("webOS video was not ready within {limit:?}"));
                    return self.fail(error, out);
                }
                self.phase = self.phase.at(Stage::Waiting {
                    since,
                    pending_since: None,
                    at: now + every,
                });
            }
            // Not preparation: no deadline runs while the TV refuses.
            Ok(Acquired::Refused) => self.refused(stage, now, out),
            Err(fault) => return self.video_fault(fault, now, out),
        }
        self.pace(now, rules, out);
    }

    /// A TV UI frame is delivered and paced, the first since a fault
    /// reported once; a fault ends the UI period.
    fn ui_acquired(
        &mut self,
        answer: Result<Frame, UiFault>,
        now: Instant,
        rules: &Rules,
        out: &mut Vec<Action>,
    ) {
        match answer {
            Ok(frame) => {
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
        }
    }
}

impl State {
    /// The driver at `stage` refused to initialise at `now`: it is kept,
    /// created, and initialised again at the next change of the video
    /// signal. The pulls are told refused, unless they were already.
    fn refused(&mut self, stage: Stage, now: Instant, out: &mut Vec<Action>) {
        let initializations = stage.refusals().map_or(1, |refusals| refusals + 1);
        let occurrence = self.refuse(now, out);
        out.push(Action::Note(Note::Refused {
            initializations,
            since_change_ms: self.since_change(now),
            occurrence,
        }));
        self.phase = self.phase.at(Stage::Refused { initializations });
    }

    /// A TV UI fault never ends the source, and ends the UI period: a held
    /// tap is released and lighting idles without video until the video or
    /// its context changes (a fault that may pass) or the next source (one
    /// that cannot).
    fn ui_fault(&mut self, fault: UiFault, now: Instant, out: &mut Vec<Action>) {
        let occurrence = self.ui.occurrence();
        self.ui = UiSupport::after_fault(fault.recovery);
        let error = fault.error;
        out.push(Action::Note(match self.ui {
            UiSupport::Unsupported => Note::UiUnsupported { error, occurrence },
            _ => Note::UiFailed { error, occurrence },
        }));
        if self.phase.holds_tap() {
            out.push(Action::Release);
        }
        self.phase = Phase::Idle;
        self.decide(now, out);
    }

    /// A video fault, as its [`Recovery`] says: one that escalates fails
    /// the source; a retry releases the tap and opens a new one at once,
    /// one time until a frame, then gives up as its [`Reason`] says; a
    /// wait for withheld video releases the tap until the signal changes.
    fn video_fault(&mut self, fault: Fault, now: Instant, out: &mut Vec<Action>) {
        let Phase::Video { stage, fresh, .. } = &mut self.phase else {
            unreachable!("a video fault answers a video period");
        };
        let held = stage.holds_tap();
        let reason = match fault.recovery {
            Recovery::Escalate(_) => return self.fail(fault.error, out),
            Recovery::Wait => return self.withhold(fault.error, now, out),
            Recovery::Retry(Reason::DriverPreparing) => {
                unreachable!("a driver not ready answers, it does not fault")
            }
            Recovery::Retry(reason @ (Reason::FreshDriver | Reason::StalledDriver)) => reason,
        };
        if fresh.failed(now).is_err() {
            return match reason.gives_up() {
                GiveUp::Fail => self.fail(fault.error, out),
                GiveUp::Wait => self.withhold(fault.error, now, out),
            };
        }
        out.push(Action::Note(Note::VideoRetrying { error: fault.error }));
        if held {
            out.push(Action::Release);
        }
        self.phase = self.phase.at(Stage::Closed);
        self.decide(now, out);
    }

    /// The TV withholds this video: the tap is released and the source
    /// idles until the signal changes. The pulls are told refused, unless
    /// they were already.
    fn withhold(&mut self, error: Error, now: Instant, out: &mut Vec<Action>) {
        // Released first, so a pull told refused finds no tap held.
        if self.phase.holds_tap() {
            out.push(Action::Release);
        }
        let occurrence = self.refuse(now, out);
        out.push(Action::Note(Note::Withheld { error, occurrence }));
        self.phase = self.phase.at(Stage::Withheld);
        self.decide(now, out);
    }

    /// End the source with `error`, the pulls no longer told refused; a
    /// held tap is released after.
    fn fail(&mut self, error: Error, out: &mut Vec<Action>) {
        self.allow(out);
        out.push(Action::Fail(error));
        if self.phase.holds_tap() {
            out.push(Action::Release);
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
        Fault {
            error: Error::other("interrupted"),
            recovery: Recovery::Retry(Reason::FreshDriver),
        }
    }

    fn ui_fault(recovery: UiRecovery) -> UiFault {
        let error = match recovery {
            UiRecovery::MayPass => Error::other("interrupted"),
            UiRecovery::Unsupported => Error::other("no library"),
        };
        UiFault { error, recovery }
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
            fresh: Attempts::new(Schedule::Once),
            refused_since: None,
        }
    }

    /// A wait for a frame since `since`, all of it pending, due at `at`.
    fn pending_wait(since: Instant, at: Instant) -> Stage {
        Stage::Waiting {
            since,
            pending_since: Some(since),
            at,
        }
    }

    /// A wait for a frame since `since` whose last answer was not ready,
    /// due at `at`.
    fn preparing_wait(since: Instant, at: Instant) -> Stage {
        Stage::Waiting {
            since,
            pending_since: None,
            at,
        }
    }

    /// A video period at `stage` whose one new driver was opened at
    /// `opened`, if it was, and whose pulls are told refused since
    /// `refused_since`.
    fn period(stage: Stage, opened: Option<Instant>, refused_since: Option<Instant>) -> Phase {
        let mut fresh = Attempts::new(Schedule::Once);
        if let Some(opened) = opened {
            fresh.failed(opened).unwrap();
        }
        Phase::Video {
            stage,
            fresh,
            refused_since,
        }
    }

    /// A video period at `stage` whose one new driver was opened at `t0`.
    fn spent(t0: Instant, stage: Stage, refused_since: Option<Instant>) -> Phase {
        period(stage, Some(t0), refused_since)
    }

    fn fault_of(recovery: Recovery) -> Fault {
        Fault {
            error: Error::other("interrupted"),
            recovery,
        }
    }

    /// A driver that refused `initializations` in a row.
    fn refused(initializations: u32) -> Stage {
        Stage::Refused { initializations }
    }

    /// A refused driver initialised again after `initializations` refusals.
    fn reinitialising(initializations: u32) -> Stage {
        Stage::Reinitialising { initializations }
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
                video(Stage::Closed),
                Action::Open(Kind::Video),
            ),
            (
                Video::Unknown,
                NO_UI,
                video(Stage::Closed),
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
        let state = at(t0, video(Stage::Closed), UiSupport::Untried);
        check(
            state,
            Event::Opened,
            t0,
            &RULES,
            video(Stage::Running),
            &[Action::Acquire],
        );
    }

    /// A video frame is delivered, ends any deadline, gives back the one
    /// new driver and ends the pulls' refused state, telling them once;
    /// then the pace.
    #[test]
    fn a_frame_is_delivered_and_paced() {
        let t0 = Instant::now();
        for stage in [Stage::Running, pending_wait(t0, t0)] {
            for withheld in [None, Some(t0)] {
                let mut expected = Vec::new();
                if withheld.is_some() {
                    expected.push(Action::Refused(false));
                    expected.push(note(Note::CaptureAllowed { elapsed_ms: 30 }));
                }
                expected.extend([
                    Action::Deliver(frame()),
                    Action::Sleep(INTERVAL - Duration::from_millis(30)),
                ]);
                check(
                    at(t0, spent(t0, stage, withheld), UiSupport::Untried),
                    acquired(Ok(Acquired::Frame(frame())), false),
                    ms(t0, 30),
                    &RULES,
                    video(Stage::Running),
                    &expected,
                );
            }
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
                Event::UiAcquired(Ok(frame.clone())),
                t0,
                &RULES,
                Phase::Ui { held: true },
                &actions,
            );
            assert_eq!(state.ui, UiSupport::Works);
        }
    }

    /// A pending driver is timed from its first answer without a frame; one
    /// that stays pending for `pending` is released and a new one opened at
    /// once. The new one stalling too, before any frame, is the TV
    /// withholding the picture: its tap is released, the pulls told
    /// refused, and the source idles.
    #[test]
    fn a_pending_driver_stalls_after_its_limit() {
        let t0 = Instant::now();
        let state = check(
            at(t0, video(Stage::Running), UiSupport::Untried),
            acquired(Ok(Acquired::Pending), false),
            t0,
            &RULES,
            video(pending_wait(t0, t0)),
            &[Action::Sleep(INTERVAL)],
        );
        let state = check(
            state,
            acquired(Ok(Acquired::Pending), false),
            ms(t0, 1999),
            &RULES,
            video(pending_wait(t0, ms(t0, 1999))),
            &[Action::Sleep(Duration::ZERO)],
        );
        let stalled = || {
            Error::other(format!(
                "webOS video produced no frame for {:?}",
                LIMITS.pending
            ))
        };
        let first = ms(t0, 2000);
        let state = check(
            state,
            acquired(Ok(Acquired::Pending), false),
            first,
            &RULES,
            spent(first, Stage::Closed, None),
            &[
                note(Note::VideoRetrying { error: stalled() }),
                Action::Release,
                Action::Open(Kind::Video),
            ],
        );
        let state = next(state, Event::Opened, first, &RULES).state;
        let state = next(state, acquired(Ok(Acquired::Pending), true), first, &RULES).state;
        let second = ms(t0, 4000);
        check(
            state,
            acquired(Ok(Acquired::Pending), false),
            second,
            &RULES,
            spent(first, Stage::Withheld, Some(second)),
            &[
                Action::Release,
                Action::Refused(true),
                note(Note::Withheld {
                    error: stalled(),
                    occurrence: Occurrence::First,
                }),
                Action::Idle,
            ],
        );
    }

    /// A driver not ready is asked again after `not_ready_retry`, and fails the source
    /// once it stayed not ready for `preparation`, its tap released after.
    #[test]
    fn a_preparing_driver_fails_after_its_limit() {
        let t0 = Instant::now();
        let preparing = |at_ms| video(preparing_wait(t0, ms(t0, at_ms)));
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
            &[Action::Fail(failed), Action::Release],
        );
    }

    /// Two clocks: `preparation` counts from the first answer without a
    /// frame of either kind, so alternating answers reach it and fail the
    /// source; `pending` counts from the first of a run of pending answers
    /// (the driver was initialised then), so a slow preparation does not
    /// shorten it.
    #[test]
    fn a_stall_counts_from_its_first_pending_answer() {
        let t0 = Instant::now();
        let preparing = |at_ms| video(preparing_wait(t0, ms(t0, at_ms)));
        let pending = |since_ms, at_ms| {
            video(Stage::Waiting {
                since: t0,
                pending_since: Some(ms(t0, since_ms)),
                at: ms(t0, at_ms),
            })
        };
        let not_ready: fn() -> Acquired = || Acquired::NotReady;
        let buffer: fn() -> Acquired = || Acquired::Pending;
        let paths = [
            // Alternating: no stall, then the preparation limit.
            vec![
                (0, not_ready, preparing(1000)),
                (1000, buffer, pending(1000, 1000)),
                (1100, not_ready, preparing(2100)),
                (2100, buffer, pending(2100, 2100)),
                (3100, buffer, pending(2100, 3100)),
                (3200, not_ready, preparing(4200)),
                (9999, not_ready, preparing(10_999)),
                (10_000, buffer, pending(10_000, 10_000)),
                (10_100, not_ready, Phase::Idle),
            ],
            // Not ready four times, then pending: a stall 2 s after the
            // first pending answer, not earlier.
            vec![
                (0, not_ready, preparing(1000)),
                (1000, not_ready, preparing(2000)),
                (2000, not_ready, preparing(3000)),
                (3000, not_ready, preparing(4000)),
                (3100, buffer, pending(3100, 3100)),
                (5099, buffer, pending(3100, 5099)),
                (5100, buffer, spent(ms(t0, 5100), Stage::Closed, None)),
            ],
        ];
        for path in paths {
            let mut state = at(t0, video(Stage::Running), UiSupport::Untried);
            for (now, answer, phase) in path {
                state = next(state, acquired(Ok(answer()), false), ms(t0, now), &RULES).state;
                assert_eq!(state.phase, phase, "at {now} ms");
            }
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
    /// counts on, as a repeat. A not-ready answer leaves the pulls refused;
    /// only the frame tells them, once.
    #[test]
    fn a_refusal_waits_for_a_change_and_lasts_until_a_frame() {
        let t0 = Instant::now();
        let since = Some(ms(t0, 100));
        let refusing = |stage| period(stage, None, since);
        let answered = |answer| {
            note(Note::InitializationAnswered {
                answer,
                since_change_ms: 100,
            })
        };
        let state = check(
            at(t0, video(Stage::Running), UiSupport::Untried),
            acquired(Ok(Acquired::Refused), true),
            ms(t0, 100),
            &RULES,
            refusing(refused(1)),
            &[
                answered(Answer::Refused),
                Action::Refused(true),
                note(Note::Refused {
                    initializations: 1,
                    since_change_ms: 100,
                    occurrence: Occurrence::First,
                }),
                Action::Sleep(Duration::ZERO),
            ],
        );
        let state = check(
            state,
            Event::Woke,
            ms(t0, 200),
            &RULES,
            refusing(refused(1)),
            &[Action::Idle],
        );
        // Only the sequence changed.
        let same = ready(Video::Available, 0, 2);
        let state = check(
            state,
            signal(same, true),
            ms(t0, 300),
            &RULES,
            refusing(reinitialising(1)),
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
            refusing(refused(2)),
            &[
                answered(Answer::Refused),
                note(Note::Refused {
                    initializations: 2,
                    since_change_ms: 100,
                    occurrence: Occurrence::Again,
                }),
                Action::Sleep(Duration::ZERO),
            ],
        );
        let changed = ready(Video::Available, 0, 3);
        let state = next(state, signal(changed, true), ms(t0, 500), &RULES).state;
        let state = check(
            state,
            acquired(Ok(Acquired::NotReady), true),
            ms(t0, 600),
            &RULES,
            refusing(preparing_wait(ms(t0, 600), ms(t0, 1600))),
            &[answered(Answer::NotReady), Action::Sleep(Duration::ZERO)],
        );
        check(
            state,
            acquired(Ok(Acquired::Frame(frame())), false),
            ms(t0, 1600),
            &RULES,
            video(Stage::Running),
            &[
                Action::Refused(false),
                note(Note::CaptureAllowed { elapsed_ms: 1500 }),
                Action::Deliver(frame()),
                Action::Sleep(Duration::ZERO),
            ],
        );
    }

    /// The pulls' refused state outlives every answer but a frame: a
    /// not-ready or pending answer and a fault that opens a new driver
    /// leave it; a frame ends it, as does the source's failure.
    #[test]
    fn only_a_frame_or_a_failure_ends_the_refused_state() {
        let t0 = Instant::now();
        let fresh = fault_of(Recovery::Retry(Reason::FreshDriver));
        for (answer, opened, told, still) in [
            (Ok(Acquired::NotReady), None, None, true),
            (Ok(Acquired::Pending), None, None, true),
            (Err(fresh), None, None, true),
            (Ok(Acquired::Frame(frame())), None, Some(false), false),
            (Err(fault()), Some(t0), Some(false), false),
        ] {
            let state = at(
                t0,
                period(reinitialising(1), opened, Some(t0)),
                UiSupport::Untried,
            );
            let step = next(state, acquired(answer, false), ms(t0, 250), &RULES);
            let refused: Vec<_> = step
                .actions
                .iter()
                .filter_map(|action| match action {
                    Action::Refused(refused) => Some(*refused),
                    _ => None,
                })
                .collect();
            assert_eq!(refused, Vec::from_iter(told), "{:?}", step.actions);
            let kept = matches!(
                step.state.phase,
                Phase::Video {
                    refused_since: Some(_),
                    ..
                }
            );
            assert_eq!(kept, still, "{:?}", step.state.phase);
        }
    }

    /// A context reset starts a new stage of the same period, its new
    /// driver given back: a created-only driver is kept (and a refusal
    /// goes on), any other held tap is released and reopened.
    #[test]
    fn a_context_reset_keeps_only_a_created_only_driver() {
        let t0 = Instant::now();
        let now = ms(t0, 50);
        let reset = ready(Video::Available, 1, 2);
        let closed = video(Stage::Closed);
        for (stage, created_only, phase, actions) in [
            (
                refused(1),
                true,
                video(reinitialising(1)),
                vec![Action::Acquire],
            ),
            (
                preparing_wait(t0, ms(t0, 900)),
                true,
                video(Stage::Running),
                vec![Action::Acquire],
            ),
            (
                Stage::Running,
                false,
                closed,
                vec![Action::Release, Action::Open(Kind::Video)],
            ),
            (
                Stage::Closed,
                false,
                closed,
                vec![Action::Open(Kind::Video)],
            ),
        ] {
            let mut expected = vec![note(Note::SignalChanged(reset))];
            if stage.refusals().is_some() {
                expected.push(note(Note::ChangedWhileRefused {
                    video: Video::Available,
                    generation: 1,
                }));
            }
            expected.extend(actions);
            check(
                at(t0, spent(t0, stage, None), UiSupport::Untried),
                signal(reset, created_only),
                now,
                &RULES,
                phase,
                &expected,
            );
        }
    }

    /// Video becoming unavailable releases a refused driver and ends the
    /// pulls' refused state.
    #[test]
    fn video_leaving_ends_a_refusal() {
        let t0 = Instant::now();
        let gone = ready(Video::Unavailable, 0, 2);
        let state = at(t0, period(refused(3), None, Some(t0)), UiSupport::Untried);
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
                note(Note::RefusalEndedWithVideo { elapsed_ms: 700 }),
                Action::Idle,
            ],
        );
    }

    /// Every video fault in every held stage, with its one new driver
    /// unspent or spent (a fault since the last frame): a retry releases
    /// the tap and opens a new one at once, one time; given up, an ordinary
    /// fault fails the source and a stall waits; a wait releases the tap,
    /// tells the pulls refused and idles; an escalation fails the source.
    #[test]
    fn every_video_fault_in_each_held_stage() {
        let t0 = Instant::now();
        let error = || Error::other("interrupted");
        let fresh = Recovery::Retry(Reason::FreshDriver);
        let stalled = Recovery::Retry(Reason::StalledDriver);
        let escalate = Recovery::Escalate(prysm_ports::Escalation::Restart);
        let retried = || {
            vec![
                note(Note::VideoRetrying { error: error() }),
                Action::Release,
                Action::Open(Kind::Video),
            ]
        };
        let failed = || vec![Action::Fail(error()), Action::Release];
        let withheld = || {
            vec![
                Action::Release,
                Action::Refused(true),
                note(Note::Withheld {
                    error: error(),
                    occurrence: Occurrence::First,
                }),
                Action::Idle,
            ]
        };
        for stage in [Stage::Running, pending_wait(t0, t0), reinitialising(1)] {
            for (recovery, opened, phase, tail) in [
                (fresh, None, spent(t0, Stage::Closed, None), retried()),
                (fresh, Some(t0), Phase::Idle, failed()),
                (stalled, None, spent(t0, Stage::Closed, None), retried()),
                (
                    stalled,
                    Some(t0),
                    spent(t0, Stage::Withheld, Some(t0)),
                    withheld(),
                ),
                (
                    Recovery::Wait,
                    None,
                    period(Stage::Withheld, None, Some(t0)),
                    withheld(),
                ),
                (
                    Recovery::Wait,
                    Some(t0),
                    spent(t0, Stage::Withheld, Some(t0)),
                    withheld(),
                ),
                (escalate, None, Phase::Idle, failed()),
            ] {
                check(
                    at(t0, period(stage, opened, None), UiSupport::Untried),
                    acquired(Err(fault_of(recovery)), false),
                    t0,
                    &RULES,
                    phase,
                    &tail,
                );
            }
        }
    }

    /// While the pulls are told refused on a new tap, only a frame or the
    /// source's failure ends it: a first retry, a not-ready and a pending
    /// answer leave the pulls refused; a failure tells the end before it
    /// fails.
    #[test]
    fn a_wait_outlives_what_is_not_a_frame() {
        let t0 = Instant::now();
        let error = || Error::other("interrupted");
        let fresh = Recovery::Retry(Reason::FreshDriver);
        let escalate = Recovery::Escalate(prysm_ports::Escalation::FAIL_FAST);
        let failed = || {
            vec![
                Action::Refused(false),
                Action::Fail(error()),
                Action::Release,
            ]
        };
        for (opened, answer, phase, expected) in [
            (
                None,
                Err(fault_of(fresh)),
                spent(t0, Stage::Closed, Some(t0)),
                vec![
                    note(Note::VideoRetrying { error: error() }),
                    Action::Release,
                    Action::Open(Kind::Video),
                ],
            ),
            (Some(t0), Err(fault_of(fresh)), Phase::Idle, failed()),
            (None, Err(fault_of(escalate)), Phase::Idle, failed()),
            (
                None,
                Ok(Acquired::Pending),
                period(pending_wait(t0, t0), None, Some(t0)),
                vec![Action::Sleep(INTERVAL)],
            ),
            (
                None,
                Ok(Acquired::NotReady),
                period(
                    preparing_wait(t0, t0 + LIMITS.not_ready_retry),
                    None,
                    Some(t0),
                ),
                vec![Action::Sleep(INTERVAL)],
            ),
        ] {
            check(
                at(
                    t0,
                    period(Stage::Running, opened, Some(t0)),
                    UiSupport::Untried,
                ),
                acquired(answer, false),
                t0,
                &RULES,
                phase,
                &expected,
            );
        }
    }

    /// An open fault holds no tap, so none is released.
    #[test]
    fn every_open_fault() {
        let t0 = Instant::now();
        let error = || Error::other("interrupted");
        let fresh = Recovery::Retry(Reason::FreshDriver);
        let failed = || vec![Action::Fail(error())];
        for (recovery, opened, phase, expected) in [
            (
                fresh,
                None,
                spent(t0, Stage::Closed, None),
                vec![
                    note(Note::VideoRetrying { error: error() }),
                    Action::Open(Kind::Video),
                ],
            ),
            (fresh, Some(t0), Phase::Idle, failed()),
            (
                Recovery::Wait,
                None,
                period(Stage::Withheld, None, Some(t0)),
                vec![
                    Action::Refused(true),
                    note(Note::Withheld {
                        error: error(),
                        occurrence: Occurrence::First,
                    }),
                    Action::Idle,
                ],
            ),
            (
                Recovery::Escalate(prysm_ports::Escalation::FAIL_FAST),
                None,
                Phase::Idle,
                failed(),
            ),
        ] {
            check(
                at(t0, period(Stage::Closed, opened, None), UiSupport::Untried),
                Event::OpenFailed(fault_of(recovery)),
                t0,
                &RULES,
                phase,
                &expected,
            );
        }
    }

    /// A wait for withheld video idles until any change of the video
    /// signal, which opens a new driver at once and keeps the wait: the
    /// pulls stay refused, and withheld again, it is not told again.
    #[test]
    fn a_wait_ends_its_idle_at_a_signal_change() {
        let t0 = Instant::now();
        let waiting = period(Stage::Withheld, None, Some(t0));
        let state = check(
            at(t0, waiting, UiSupport::Untried),
            Event::Woke,
            ms(t0, 100),
            &RULES,
            waiting,
            &[Action::Idle],
        );
        let same = ready(Video::Available, 0, 2);
        let reopened = period(Stage::Closed, None, Some(t0));
        let state = check(
            state,
            signal(same, false),
            ms(t0, 200),
            &RULES,
            reopened,
            &[
                note(Note::SignalChanged(same)),
                note(Note::ChangedWhileWithheld {
                    video: Video::Available,
                    generation: 0,
                }),
                Action::Open(Kind::Video),
            ],
        );
        // A context reset reopens the same way.
        let reset = ready(Video::Available, 1, 2);
        check(
            at(t0, waiting, UiSupport::Untried),
            signal(reset, false),
            ms(t0, 200),
            &RULES,
            reopened,
            &[
                note(Note::SignalChanged(reset)),
                note(Note::ChangedWhileWithheld {
                    video: Video::Available,
                    generation: 1,
                }),
                Action::Open(Kind::Video),
            ],
        );
        let state = next(state, Event::Opened, ms(t0, 200), &RULES).state;
        check(
            state,
            acquired(Err(fault_of(Recovery::Wait)), true),
            ms(t0, 300),
            &RULES,
            waiting,
            &[
                note(Note::InitializationAnswered {
                    answer: Answer::Failed,
                    since_change_ms: 100,
                }),
                Action::Release,
                note(Note::Withheld {
                    error: Error::other("interrupted"),
                    occurrence: Occurrence::Again,
                }),
                Action::Idle,
            ],
        );
    }

    /// A change of tap kind ends the refused state: the pulls are told once, and
    /// nothing is released while the wait holds no tap. A stop during the
    /// wait releases nothing either.
    #[test]
    fn a_kind_change_or_a_stop_ends_a_wait_without_a_release() {
        let t0 = Instant::now();
        let gone = ready(Video::Unavailable, 0, 2);
        for (stage, release) in [(Stage::Withheld, false), (Stage::Running, true)] {
            let mut expected = vec![
                note(Note::SignalChanged(gone)),
                note(Note::VideoChanged(Video::Unavailable)),
            ];
            if release {
                expected.push(Action::Release);
            } else {
                expected.push(note(Note::ChangedWhileWithheld {
                    video: Video::Unavailable,
                    generation: 0,
                }));
            }
            expected.extend([
                Action::Refused(false),
                note(Note::RefusalEndedWithVideo { elapsed_ms: 400 }),
                Action::Idle,
            ]);
            check(
                at(t0, spent(t0, stage, Some(t0)), UiSupport::Untried),
                signal(gone, false),
                ms(t0, 400),
                &NO_UI,
                Phase::Idle,
                &expected,
            );
        }
        check(
            at(t0, spent(t0, Stage::Withheld, Some(t0)), UiSupport::Untried),
            Event::Stop,
            t0,
            &RULES,
            Phase::Idle,
            &[],
        );
    }

    /// A frame between two ordinary faults gives the one new driver back:
    /// the second fault opens a new one again instead of failing.
    #[test]
    fn a_frame_between_two_faults_gives_the_new_driver_back() {
        let t0 = Instant::now();
        let fresh = || fault_of(Recovery::Retry(Reason::FreshDriver));
        let mut state = at(t0, video(Stage::Running), UiSupport::Untried);
        for _ in 0..2 {
            state = next(state, acquired(Err(fresh()), false), t0, &RULES).state;
            assert_eq!(state.phase, spent(t0, Stage::Closed, None));
            state = next(state, Event::Opened, t0, &RULES).state;
            state = next(
                state,
                acquired(Ok(Acquired::Frame(frame())), true),
                t0,
                &RULES,
            )
            .state;
            assert_eq!(state.phase, video(Stage::Running));
        }
        let step = next(state, acquired(Err(fresh()), false), t0, &RULES);
        assert_eq!(step.state.phase, spent(t0, Stage::Closed, None));
        let failed = next(step.state, Event::OpenFailed(fresh()), t0, &RULES);
        assert_eq!(failed.state.phase, Phase::Idle);
        assert!(matches!(failed.actions[..], [Action::Fail(_)]));
    }

    /// Runs `events` from `state`, each at its milliseconds after `t0`;
    /// returns the last state and every action.
    fn run(
        t0: Instant,
        state: State,
        events: Vec<(u64, Event)>,
        rules: &Rules,
    ) -> (State, Vec<Action>) {
        events.into_iter().fold(
            (state, Vec::new()),
            |(state, mut actions), (at_ms, event)| {
                let step = next(state, event, ms(t0, at_ms), rules);
                actions.extend(step.actions);
                (step.state, actions)
            },
        )
    }

    fn refused_told(actions: &[Action]) -> Vec<bool> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Refused(refused) => Some(*refused),
                _ => None,
            })
            .collect()
    }

    /// One refused state per video period: withheld (13), then refused
    /// (17) on the new driver, then not ready, keeps the pulls refused and
    /// tells them once; the refusal after the withholding is a repeat. The
    /// frame tells the end once.
    #[test]
    fn a_withholding_and_a_refusal_are_one_refused_state() {
        let t0 = Instant::now();
        let change =
            |sequence, created_only| signal(ready(Video::Available, 0, sequence), created_only);
        let (state, actions) = run(
            t0,
            at(t0, video(Stage::Running), UiSupport::Untried),
            vec![
                (0, acquired(Err(fault_of(Recovery::Wait)), false)),
                (100, change(2, false)),
                (100, Event::Opened),
                (200, acquired(Ok(Acquired::Refused), true)),
                (300, change(3, true)),
                (300, acquired(Ok(Acquired::NotReady), true)),
            ],
            &RULES,
        );
        assert_eq!(refused_told(&actions), [true]);
        assert!(step_notes(&actions).iter().any(|note| matches!(
            note,
            Note::Refused {
                occurrence: Occurrence::Again,
                ..
            }
        )));
        assert!(matches!(
            state.phase,
            Phase::Video {
                stage: Stage::Waiting { .. },
                refused_since: Some(since),
                ..
            } if since == t0
        ));
        let (state, actions) = run(
            t0,
            state,
            vec![(400, acquired(Ok(Acquired::Frame(frame())), false))],
            &RULES,
        );
        assert_eq!(refused_told(&actions), [false]);
        assert!(matches!(
            state.phase,
            Phase::Video {
                refused_since: None,
                ..
            }
        ));
    }

    /// A change of the video signal gives the one new driver back: after
    /// two stalls withheld the video, a change opens a new driver, and one
    /// ordinary fault of it opens another instead of failing; a second
    /// fault with no frame or change between fails the source.
    #[test]
    fn a_signal_change_gives_the_new_driver_back() {
        let t0 = Instant::now();
        let pending = || acquired(Ok(Acquired::Pending), false);
        let (state, actions) = run(
            t0,
            at(t0, video(Stage::Running), UiSupport::Untried),
            vec![
                (0, pending()),
                (2000, pending()),
                (2000, Event::Opened),
                (2000, acquired(Ok(Acquired::Pending), true)),
                (4000, pending()),
            ],
            &RULES,
        );
        assert_eq!(
            state.phase,
            spent(ms(t0, 2000), Stage::Withheld, Some(ms(t0, 4000)))
        );
        assert_eq!(refused_told(&actions), [true]);
        let (state, _) = run(
            t0,
            state,
            vec![
                (5000, signal(ready(Video::Available, 0, 2), false)),
                (5000, Event::Opened),
            ],
            &RULES,
        );
        let error = || Error::other("interrupted");
        let state = check(
            state,
            acquired(Err(fault()), true),
            ms(t0, 5100),
            &RULES,
            spent(ms(t0, 5100), Stage::Closed, Some(ms(t0, 4000))),
            &[
                note(Note::InitializationAnswered {
                    answer: Answer::Failed,
                    since_change_ms: 100,
                }),
                note(Note::VideoRetrying { error: error() }),
                Action::Release,
                Action::Open(Kind::Video),
            ],
        );
        check(
            state,
            Event::OpenFailed(fault()),
            ms(t0, 5200),
            &RULES,
            Phase::Idle,
            &[Action::Refused(false), Action::Fail(error())],
        );
    }

    /// A stop releases the tap iff one is held.
    #[test]
    fn a_stop_releases_only_a_held_tap() {
        let t0 = Instant::now();
        let closed = Stage::Closed;
        let held = [
            Stage::Running,
            pending_wait(t0, t0),
            refused(1),
            reinitialising(1),
        ];
        let mut cases = vec![
            (Phase::Idle, false),
            (video(closed), false),
            (Phase::Ui { held: false }, false),
            (Phase::Ui { held: true }, true),
        ];
        cases.extend(held.map(|stage| (video(stage), true)));
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
            Event::UiAcquired(Err(ui_fault(UiRecovery::MayPass))),
            t0,
            &RULES,
            Phase::Idle,
            &[ui_failed(Occurrence::First), Action::Release, Action::Idle],
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
                Action::Release,
                Action::Open(Kind::Ui),
            ],
        );
        let state = check(
            state,
            Event::UiOpenFailed(ui_fault(UiRecovery::MayPass)),
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
        let fault = ui_fault(UiRecovery::Unsupported);
        let error = fault.error.clone();
        let state = next(
            at(t0, Phase::Ui { held: false }, UiSupport::Untried),
            Event::UiOpenFailed(fault),
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
            video(Stage::Closed),
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
        let refused = at(t0, period(refused(1), None, Some(t0)), UiSupport::Untried);
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
        assert_eq!(UiSupport::after_fault(UiRecovery::Unsupported), Unsupported);
        assert_eq!(UiSupport::after_fault(UiRecovery::MayPass), Failed);
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
        for (held, recovery, ui) in [
            (false, UiRecovery::MayPass, UiSupport::Failed),
            (true, UiRecovery::MayPass, UiSupport::Failed),
            (false, UiRecovery::Unsupported, UiSupport::Unsupported),
            (true, UiRecovery::Unsupported, UiSupport::Unsupported),
        ] {
            let fault = ui_fault(recovery);
            let error = fault.error.clone();
            let event = if held {
                Event::UiAcquired(Err(fault))
            } else {
                Event::UiOpenFailed(fault)
            };
            let occurrence = Occurrence::First;
            let mut actions = vec![note(match ui {
                UiSupport::Unsupported => Note::UiUnsupported { error, occurrence },
                _ => Note::UiFailed { error, occurrence },
            })];
            if held {
                actions.push(Action::Release);
            }
            actions.push(Action::Idle);
            let state = at(t0, Phase::Ui { held }, UiSupport::Works);
            let after = check(state, event, t0, &RULES, Phase::Idle, &actions);
            assert_eq!(after.ui, ui, "{held}");
        }
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
