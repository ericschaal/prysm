//! Pure transitions for the native capture thread.
use super::{Acquired, Fault, Limits, UiFault};
use crate::classify::{Reason, Recovery, UiRecovery, VideoCause};
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

/// UI capture availability and fault reporting for the current source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UiSupport {
    Untried,
    Works,
    /// Failed in a way that may pass: not opened until the video or its
    /// context changes.
    Failed,
    /// May be opened again after a failure; its next fault is no news.
    Reopenable,
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

/// Where a video tap's period stands. The stage says whether the held
/// driver is created-only ([`Stage::created_only`]): the driver's last
/// answer decides it, and the machine sees every answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    /// No tap: opened at once.
    Closed,
    /// A created-only driver none of whose answers counts: just opened, or
    /// found not ready by a change of its video context, which restarts
    /// the TV's video pipeline and may make the driver ready. Initialized
    /// at once; [`Limits::preparation`] counts from its next answer.
    Created,
    /// An initialized driver that delivered a frame: acquired at the pace.
    Running,
    /// A held driver with no frame since `since`, the first answer without
    /// one (not ready or a pending buffer) since it was [`Stage::Created`]
    /// or last delivered a frame: the preparation limit counts from it.
    /// `pending_since` is the first of the pending answers since the driver
    /// last answered anything else (it was initialized then): the stall
    /// counts from it. `None` after a not-ready answer, which leaves the
    /// driver created-only. Acquired again at `at`:
    /// [`Limits::not_ready_retry`] after a not-ready answer, at the pace
    /// after a pending one.
    NoFrame {
        since: Instant,
        pending_since: Option<Instant>,
        at: Instant,
    },
    /// The driver refused its last `initializations` in a row: initialized
    /// again at the next change of the video signal, since nothing tells
    /// when the TV would allow it sooner. A refused driver stays
    /// created-only, so this stage always keeps its tap.
    Refused { initializations: u32 },
    /// No tap: the TV withholds this video ([`Recovery::Withheld`]). Opened
    /// again at the next change of the video signal, since nothing tells
    /// when the TV would give it sooner.
    Withheld,
}

/// The one new driver a video fault that retries opens
/// ([`crate::classify::Reason`]); a frame or a change of the video signal
/// gives it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NewDriver {
    /// The next retrying fault opens it.
    Available,
    /// Opened since the last frame or change of the video signal: the next
    /// retrying fault gives up.
    Spent,
    /// This driver was reopened after a stall.
    ReopenedAfterStall,
}

/// The held video driver and its recovery history for this video period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VideoPeriod {
    stage: Stage,
    new_driver: NewDriver,
    /// Since when the pulls are told refused: from the period's first
    /// refusal or withholding until a frame, the end of the period (a
    /// change of tap kind) or the source's failure. Nothing else ends
    /// it, and it is the only source of [`Action::TellRefused`].
    refused_since: Option<Instant>,
}

impl VideoPeriod {
    /// The video period's pulls are told refused from `now`, unless they
    /// were already: the first refusal or withholding since a frame is
    /// news.
    fn refuse(&mut self, now: Instant, actions: &mut Vec<Action>) -> Occurrence {
        if self.refused_since.is_some() {
            return Occurrence::Again;
        }
        self.refused_since = Some(now);
        actions.push(Action::TellRefused(true));
        Occurrence::First
    }
}

/// The tap the native thread's current period is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    /// No tap wanted: until the video signal changes.
    Idle,
    Video(VideoPeriod),
    /// The TV UI tap, `held` once opened. It answers only a frame or a
    /// fault (its driver has no separate initialization), so it has none of
    /// the video tap's stages: a fault ends its period.
    Ui {
        held: bool,
    },
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
    /// When the next paced step is due: one interval after the last one
    /// was, or at once if that has passed.
    tick: Instant,
}

/// What the machine is told in a run of the loop.
#[derive(Debug)]
pub(super) enum Event {
    /// A change of the video signal: the signal read has a new sequence,
    /// which every `set` and `reset` bumps, a `set` to the same value
    /// included. Only a change of the video (its availability) or of its
    /// context (its generation) is news for the TV UI tap.
    Signal(Readiness),
    /// The tap opened after [`Next::Open`].
    Opened,
    OpenFailed(Fault),
    UiOpenFailed(UiFault),
    /// A video acquisition answered.
    Acquired(Result<Acquired, Fault>),
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
    Release,
    Deliver(Frame),
    /// Tell the pulls whether the TV refuses.
    TellRefused(bool),
    /// The replacement stalled. The release preceding this action succeeded.
    ReportStall(Error),
    /// A native tap initialized successfully.
    Initialized,
    Note(Note),
    /// Publish this failure and end; a held tap is then released.
    Fail(Error),
}

/// What a video acquisition says of the driver's initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Answer {
    /// `vtCapture_init` refused (status 17).
    Refused,
    /// Not ready yet (status 1).
    NotReady,
    /// Initialized: a frame or a pending buffer.
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
    /// The signal changed while the driver is refused: it is initialized
    /// again.
    ChangedWhileRefused { video: Video, generation: u64 },
    /// An initialization of a created-only driver answered.
    InitializationAnswered {
        answer: Answer,
        since_change_ms: u128,
    },
    /// The driver refused an initialization, its `initializations`-th in
    /// a row; the first refusal or withholding since a frame is news.
    Refused {
        initializations: u32,
        since_change_ms: u128,
        occurrence: Occurrence,
    },
    /// `by` ended the pulls' refused state of `elapsed_ms`.
    RefusalEnded { by: RefusalEnd, elapsed_ms: u128 },
    /// The first TV UI frame of the source or since a UI fault.
    UiCapturing { width: u32, height: u32 },
    /// A TV UI fault, which may pass or cannot heal as `recovery` says.
    UiFault {
        recovery: UiRecovery,
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

/// What ended the pulls' refused state, as a [`Note::RefusalEnded`] says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefusalEnd {
    Frame,
    /// A change of tap kind: the video left.
    KindChange,
}

/// What the decisions depend on besides the state.
#[derive(Debug, Clone, Copy)]
pub(super) struct Rules {
    pub limits: Limits,
    pub include_ui: bool,
    /// Between two steps.
    pub interval: Duration,
}

/// The shell runs this after the actions. Native calls feed their answers straight
/// back to the machine. Waking from a wait checks for a stop or a new signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Next {
    /// Sleep up to this duration, then read the stop flag and latest signal.
    Sleep(Duration),
    /// Wait for the signal or a stop.
    Idle,
    End,
    /// Answered with [`Event::Opened`], [`Event::OpenFailed`] or
    /// [`Event::UiOpenFailed`].
    Open(Kind),
    /// Answered with [`Event::Acquired`] or [`Event::UiAcquired`].
    Acquire,
}

#[derive(Debug)]
pub(super) struct Step {
    pub state: State,
    pub actions: Vec<Action>,
    pub next: Next,
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
    fn after_video_or_context_change(self) -> Self {
        match self {
            Self::Failed => Self::Reopenable,
            other => other,
        }
    }

    fn may_open(self) -> bool {
        matches!(self, Self::Untried | Self::Works | Self::Reopenable)
    }

    /// How a fault in this state is logged: one since the last UI frame is
    /// no news.
    fn occurrence(self) -> Occurrence {
        match self {
            Self::Untried | Self::Works => Occurrence::First,
            Self::Failed | Self::Reopenable | Self::Unsupported => Occurrence::Again,
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

impl Stage {
    fn holds_tap(self) -> bool {
        match self {
            Self::Closed | Self::Withheld => false,
            Self::Created | Self::Running | Self::NoFrame { .. } | Self::Refused { .. } => true,
        }
    }

    /// The held driver is created and not initialized: just opened, or its
    /// last answer was not ready or refused. Releasing it undoes only its
    /// creation, and its next acquisition initializes it.
    fn created_only(self) -> bool {
        match self {
            Self::Created
            | Self::NoFrame {
                pending_since: None,
                ..
            }
            | Self::Refused { .. } => true,
            Self::Closed
            | Self::Running
            | Self::NoFrame {
                pending_since: Some(_),
                ..
            }
            | Self::Withheld => false,
        }
    }

    /// The refusals in a row of a refused driver; none in any other stage.
    fn refusals(self) -> Option<u32> {
        match self {
            Self::Refused { initializations } => Some(initializations),
            Self::Closed
            | Self::Created
            | Self::Running
            | Self::NoFrame { .. }
            | Self::Withheld => None,
        }
    }

    /// When the stretch without a frame started: kept in
    /// [`Stage::NoFrame`], else `now`.
    fn no_frame_since(self, now: Instant) -> Instant {
        match self {
            Self::NoFrame { since, .. } => since,
            _ => now,
        }
    }

    /// When the run of pending answers started: kept while it runs, else
    /// `now`.
    fn pending_since(self, now: Instant) -> Instant {
        match self {
            Self::NoFrame {
                pending_since: Some(since),
                ..
            } => since,
            _ => now,
        }
    }
}

impl Phase {
    /// A new period for `kind`, with no tap: a video period has its new
    /// driver to open and the pulls not told refused.
    fn begin(kind: Option<Kind>) -> Self {
        match kind {
            None => Self::Idle,
            Some(Kind::Video) => Self::Video(VideoPeriod {
                stage: Stage::Closed,
                new_driver: NewDriver::Available,
                refused_since: None,
            }),
            Some(Kind::Ui) => Self::Ui { held: false },
        }
    }

    fn kind(self) -> Option<Kind> {
        match self {
            Self::Idle => None,
            Self::Video(_) => Some(Kind::Video),
            Self::Ui { .. } => Some(Kind::Ui),
        }
    }

    fn holds_tap(self) -> bool {
        match self {
            Self::Idle => false,
            Self::Video(VideoPeriod { stage, .. }) => stage.holds_tap(),
            Self::Ui { held } => held,
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
    let next = match event {
        Event::Signal(readiness) => {
            let context_changed = state.signal(readiness, now, &mut actions);
            state.choose_tap(readiness, context_changed, now, rules, &mut actions)
        }
        Event::Woke => state.decide(now),
        Event::Opened => state.opened(&mut actions),
        Event::OpenFailed(fault) => {
            let Phase::Video(video) = &mut state.phase else {
                unreachable!("a video fault answers a video period");
            };
            video.fault(fault, now, &mut actions)
        }
        Event::UiOpenFailed(fault) => state.ui_fault(fault, now, &mut actions),
        Event::Acquired(answer) => {
            let since_change_ms = state.since_change(now);
            let Phase::Video(video) = &mut state.phase else {
                unreachable!("only a video period has a driver");
            };
            match video.acquired(answer, now, rules.limits, since_change_ms, &mut actions) {
                Ok(()) => state.pace(now, rules),
                Err(fault) => video.fault(fault, now, &mut actions),
            }
        }
        Event::UiAcquired(answer) => state.ui_acquired(answer, now, rules, &mut actions),
        Event::Stop => {
            if state.phase.holds_tap() {
                actions.push(Action::Release);
            }
            Next::End
        }
    };
    if next == Next::End {
        state.phase = Phase::Idle;
    }
    Step {
        state,
        actions,
        next,
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
    /// video). A change is new evidence, so it gives the one new driver
    /// back; only a change of the video or its context lets a failed TV UI
    /// be tried again. Returns whether the context changed.
    fn signal(&mut self, readiness: Readiness, now: Instant, actions: &mut Vec<Action>) -> bool {
        let before = self.seen.replace(Seen { readiness, at: now });
        actions.push(Action::Note(Note::SignalChanged(readiness)));
        let video_changed = before.is_none_or(|before| before.readiness.video != readiness.video);
        if video_changed {
            actions.push(Action::Note(Note::VideoChanged(readiness.video)));
        }
        let context_changed =
            before.is_some_and(|before| before.readiness.generation != readiness.generation);
        if video_changed || context_changed {
            self.ui = self.ui.after_video_or_context_change();
        }
        if let Phase::Video(VideoPeriod { new_driver, .. }) = &mut self.phase {
            *new_driver = NewDriver::Available;
        }
        context_changed
    }

    /// The tap kind the signal wants, decided first, then what is due. A
    /// change of kind starts a new period, releasing the held tap and
    /// ending the refused state. Otherwise the period goes on: a refused
    /// driver is initialized once more at once, its stage still
    /// [`Stage::Refused`] until it answers; withheld video gets a new
    /// driver; and a change of the video context releases and reopens a
    /// held tap, except a created-only one: it holds no video context and
    /// its release can hang, so it is [`Stage::Created`]. A refused driver
    /// stays created-only, so it is always kept.
    fn choose_tap(
        &mut self,
        readiness: Readiness,
        context_changed: bool,
        now: Instant,
        rules: &Rules,
        actions: &mut Vec<Action>,
    ) -> Next {
        let Readiness {
            video, generation, ..
        } = readiness;
        let kind = wanted(video, rules.include_ui, self.ui);
        if kind != self.phase.kind() {
            if self.phase.holds_tap() {
                actions.push(Action::Release);
            }
            if let Phase::Video(period) = &mut self.phase {
                period.end_refusal(RefusalEnd::KindChange, now, actions);
            }
            self.phase = Phase::begin(kind);
            return self.decide(now);
        }
        match &mut self.phase {
            Phase::Video(period) => match period.stage {
                Stage::Refused { .. } => {
                    actions.push(Action::Note(Note::ChangedWhileRefused {
                        video,
                        generation,
                    }));
                    return Next::Acquire;
                }
                Stage::Withheld => {
                    actions.push(Action::Note(Note::ChangedWhileWithheld {
                        video,
                        generation,
                    }));
                    period.stage = Stage::Closed;
                }
                stage if context_changed && stage.holds_tap() => {
                    period.stage = if stage.created_only() {
                        Stage::Created
                    } else {
                        actions.push(Action::Release);
                        Stage::Closed
                    };
                }
                _ => {}
            },
            Phase::Ui { held: true } if context_changed => {
                actions.push(Action::Release);
                self.phase = Phase::Ui { held: false };
            }
            Phase::Ui { .. } | Phase::Idle => {}
        }
        self.decide(now)
    }

    /// Idle without a tap wanted, while refused or withheld (until a
    /// signal, [`Self::choose_tap`]); sleep until a deadline not reached;
    /// else ask to open or acquire.
    fn decide(&self, now: Instant) -> Next {
        match self.phase {
            Phase::Idle => Next::Idle,
            Phase::Ui { held: false } => Next::Open(Kind::Ui),
            Phase::Ui { held: true } => Next::Acquire,
            Phase::Video(period) => period.decide(now),
        }
    }

    /// The tap opened: acquire at once, in the same step.
    fn opened(&mut self, actions: &mut Vec<Action>) -> Next {
        match &mut self.phase {
            Phase::Video(period) if period.stage == Stage::Closed => period.stage = Stage::Created,
            Phase::Ui { held } if !*held => {
                *held = true;
                actions.push(Action::Initialized);
            }
            _ => unreachable!("only a tap asked for opens"),
        }
        Next::Acquire
    }

    /// A TV UI frame is delivered and paced, the first since a fault
    /// reported once; a fault ends the UI period.
    fn ui_acquired(
        &mut self,
        answer: Result<Frame, UiFault>,
        now: Instant,
        rules: &Rules,
        actions: &mut Vec<Action>,
    ) -> Next {
        match answer {
            Ok(frame) => {
                if self.ui != UiSupport::Works {
                    self.ui = UiSupport::Works;
                    actions.push(Action::Note(Note::UiCapturing {
                        width: frame.width(),
                        height: frame.height(),
                    }));
                }
                actions.push(Action::Deliver(frame));
                self.pace(now, rules)
            }
            Err(fault) => self.ui_fault(fault, now, actions),
        }
    }

    /// A TV UI fault never ends the source, and ends the UI period: a held
    /// tap is released and lighting idles without video until the video or
    /// its context changes (a fault that may pass) or the next source (one
    /// that cannot).
    fn ui_fault(&mut self, fault: UiFault, now: Instant, actions: &mut Vec<Action>) -> Next {
        let occurrence = self.ui.occurrence();
        self.ui = UiSupport::after_fault(fault.recovery);
        actions.push(Action::Note(Note::UiFault {
            recovery: fault.recovery,
            error: fault.error,
            occurrence,
        }));
        if self.phase.holds_tap() {
            actions.push(Action::Release);
        }
        self.phase = Phase::Idle;
        self.decide(now)
    }

    /// Pace on a fixed clock after a step; skip missed ticks instead of
    /// bursting.
    fn pace(&mut self, now: Instant, rules: &Rules) -> Next {
        self.tick += rules.interval;
        if now > self.tick {
            self.tick = now;
        }
        Next::Sleep(self.tick - now)
    }
}

impl VideoPeriod {
    fn decide(&self, now: Instant) -> Next {
        match self.stage {
            Stage::Refused { .. } | Stage::Withheld => Next::Idle,
            Stage::NoFrame { at, .. } if now < at => Next::Sleep(at - now),
            Stage::Closed => Next::Open(Kind::Video),
            Stage::NoFrame { .. } | Stage::Created | Stage::Running => Next::Acquire,
        }
    }

    /// The pulls stop being told refused, if they were: returns since when
    /// they were.
    fn allow(&mut self, actions: &mut Vec<Action>) -> Option<Instant> {
        let since = self.refused_since.take()?;
        actions.push(Action::TellRefused(false));
        Some(since)
    }

    /// The pulls stop being told refused, if they were, and `by` is noted
    /// with how long they were.
    fn end_refusal(&mut self, by: RefusalEnd, now: Instant, actions: &mut Vec<Action>) {
        if let Some(since) = self.allow(actions) {
            actions.push(Action::Note(Note::RefusalEnded {
                by,
                elapsed_ms: millis(since, now),
            }));
        }
    }

    /// Act on one video acquisition, which initialized the driver if it was
    /// created-only: publish a frame, time the wait for one, keep a refused
    /// driver, or return its fault for recovery.
    fn acquired(
        &mut self,
        answer: Result<Acquired, Fault>,
        now: Instant,
        limits: Limits,
        since_change_ms: u128,
        actions: &mut Vec<Action>,
    ) -> Result<(), Fault> {
        let stage = self.stage;
        let answered = |answer, actions: &mut Vec<Action>| {
            if stage.created_only() {
                actions.push(Action::Note(Note::InitializationAnswered {
                    answer,
                    since_change_ms,
                }));
                if answer == Answer::Accepted {
                    actions.push(Action::Initialized);
                }
            }
        };
        match answer {
            Ok(Acquired::Frame(frame)) => {
                answered(Answer::Accepted, actions);
                self.new_driver = NewDriver::Available;
                self.end_refusal(RefusalEnd::Frame, now, actions);
                self.stage = Stage::Running;
                actions.push(Action::Deliver(frame));
            }
            // Stalled once pending for `pending` since the driver was
            // initialized.
            Ok(Acquired::Pending) => {
                answered(Answer::Accepted, actions);
                let pending_since = stage.pending_since(now);
                let limit = limits.pending;
                if now.duration_since(pending_since) >= limit {
                    let context = format!("webOS video produced no frame for {limit:?}");
                    return Err(Fault::limit(context, VideoCause::Stall));
                }
                self.stage = Stage::NoFrame {
                    since: stage.no_frame_since(now),
                    pending_since: Some(pending_since),
                    at: now,
                };
            }
            // Timed on the whole wait for a frame, which a pending answer
            // may have started, so alternating answers cannot postpone it;
            // only a new tap or a change of the video context
            // ([`Stage::Created`]) starts it again.
            Ok(Acquired::NotReady) => {
                answered(Answer::NotReady, actions);
                let limit = limits.preparation;
                let since = stage.no_frame_since(now);
                if now.duration_since(since) >= limit {
                    let context = format!("webOS video was not ready within {limit:?}");
                    let unprepared = Fault::limit(context, VideoCause::Unprepared);
                    return Err(unprepared);
                }
                self.stage = Stage::NoFrame {
                    since,
                    pending_since: None,
                    at: now + limits.not_ready_retry,
                };
            }
            // Not preparation: no deadline runs while the TV refuses.
            Ok(Acquired::Refused) => {
                answered(Answer::Refused, actions);
                self.refused(now, since_change_ms, actions);
            }
            Err(fault) => {
                answered(Answer::Failed, actions);
                return Err(fault);
            }
        }
        Ok(())
    }

    /// The driver refused to initialize at `now`: it is kept,
    /// created, and initialized again at the next change of the video
    /// signal. The pulls are told refused, unless they were already.
    fn refused(&mut self, now: Instant, since_change_ms: u128, actions: &mut Vec<Action>) {
        let initializations = self.stage.refusals().map_or(1, |refusals| refusals + 1);
        let occurrence = self.refuse(now, actions);
        actions.push(Action::Note(Note::Refused {
            initializations,
            since_change_ms,
            occurrence,
        }));
        self.stage = Stage::Refused { initializations };
    }

    /// A video fault, as its [`Recovery`] says: one that escalates fails
    /// the source; a retry releases the tap and opens a new one at once,
    /// one time until a frame, then gives up as its
    /// [`crate::classify::Reason`] says; withheld video releases the tap
    /// until the signal changes.
    fn fault(&mut self, fault: Fault, now: Instant, actions: &mut Vec<Action>) -> Next {
        let held = self.stage.holds_tap();
        let reason = match fault.recovery {
            Recovery::Escalate(_) => return self.fail(fault.error, actions),
            Recovery::Withheld => return self.withhold(fault.error, now, actions),
            Recovery::Retry(reason) => reason,
        };
        match (self.new_driver, reason) {
            (NewDriver::ReopenedAfterStall, Reason::StalledDriver) => {
                if held {
                    actions.push(Action::Release);
                }
                self.allow(actions);
                actions.push(Action::ReportStall(fault.error));
                return Next::End;
            }
            (_, Reason::StalledDriver) => self.new_driver = NewDriver::ReopenedAfterStall,
            (NewDriver::Available, Reason::FreshDriver) => self.new_driver = NewDriver::Spent,
            (NewDriver::Spent | NewDriver::ReopenedAfterStall, Reason::FreshDriver) => {
                return self.fail(fault.error, actions);
            }
        }
        actions.push(Action::Note(Note::VideoRetrying { error: fault.error }));
        if held {
            actions.push(Action::Release);
        }
        self.stage = Stage::Closed;
        self.decide(now)
    }

    /// The TV withholds this video: the tap is released and the source
    /// idles until the signal changes. The pulls are told refused, unless
    /// they were already.
    fn withhold(&mut self, error: Error, now: Instant, actions: &mut Vec<Action>) -> Next {
        // Released first, so a pull told refused finds no tap held.
        if self.stage.holds_tap() {
            actions.push(Action::Release);
        }
        let occurrence = self.refuse(now, actions);
        actions.push(Action::Note(Note::Withheld { error, occurrence }));
        self.stage = Stage::Withheld;
        self.decide(now)
    }

    /// End the source with `error`, the pulls no longer told refused; a
    /// held tap is released after.
    fn fail(&mut self, error: Error, actions: &mut Vec<Action>) -> Next {
        self.allow(actions);
        actions.push(Action::Fail(error));
        if self.stage.holds_tap() {
            actions.push(Action::Release);
        }
        Next::End
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
        Phase::Video(VideoPeriod {
            stage,
            new_driver: NewDriver::Available,
            refused_since: None,
        })
    }

    /// No frame since `since`, all of it pending, due at `at`.
    fn no_frame_pending(since: Instant, at: Instant) -> Stage {
        Stage::NoFrame {
            since,
            pending_since: Some(since),
            at,
        }
    }

    /// No frame since `since`, the last answer not ready,
    /// due at `at`.
    fn no_frame_not_ready(since: Instant, at: Instant) -> Stage {
        Stage::NoFrame {
            since,
            pending_since: None,
            at,
        }
    }

    /// A video period at `stage` with its one new driver as `new_driver`,
    /// whose pulls are told refused since `refused_since`.
    fn period(stage: Stage, new_driver: NewDriver, refused_since: Option<Instant>) -> Phase {
        Phase::Video(VideoPeriod {
            stage,
            new_driver,
            refused_since,
        })
    }

    /// A video period at `stage` whose one new driver was opened.
    fn spent(stage: Stage, refused_since: Option<Instant>) -> Phase {
        period(stage, NewDriver::Spent, refused_since)
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

    fn check(
        state: State,
        event: Event,
        now: Instant,
        rules: &Rules,
        phase: Phase,
        actions: &[Action],
        operation: Next,
    ) -> State {
        let step = next(state, event, now, rules);
        assert_eq!(step.state.phase, phase, "phase after {state:?}");
        assert_eq!(
            format!("{:?}", step.actions),
            format!("{actions:?}"),
            "actions from {state:?}"
        );
        assert_eq!(step.next, operation, "next operation from {state:?}");
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
        for (video, rules, phase, operation) in [
            (
                Video::Available,
                RULES,
                video(Stage::Closed),
                Next::Open(Kind::Video),
            ),
            (
                Video::Unknown,
                NO_UI,
                video(Stage::Closed),
                Next::Open(Kind::Video),
            ),
            (
                Video::Unavailable,
                RULES,
                Phase::Ui { held: false },
                Next::Open(Kind::Ui),
            ),
            (Video::Unavailable, NO_UI, Phase::Idle, Next::Idle),
        ] {
            let readiness = ready(video, 3, 7);
            let actions = [
                note(Note::SignalChanged(readiness)),
                note(Note::VideoChanged(video)),
            ];
            let state = check(
                State::new(t0),
                Event::Signal(readiness),
                t0,
                &rules,
                phase,
                &actions,
                operation,
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
            video(Stage::Created),
            &[],
            Next::Acquire,
        );
    }

    /// A video frame is delivered, ends any deadline, gives back the one
    /// new driver and ends the pulls' refused state, telling them once;
    /// then the pace.
    #[test]
    fn a_frame_is_delivered_and_paced() {
        let t0 = Instant::now();
        for stage in [Stage::Running, no_frame_pending(t0, t0)] {
            for withheld in [None, Some(t0)] {
                let mut expected = Vec::new();
                if withheld.is_some() {
                    expected.push(Action::TellRefused(false));
                    expected.push(note(Note::RefusalEnded {
                        by: RefusalEnd::Frame,
                        elapsed_ms: 30,
                    }));
                }
                expected.push(Action::Deliver(frame()));
                check(
                    at(t0, spent(stage, withheld), UiSupport::Untried),
                    Event::Acquired(Ok(Acquired::Frame(frame()))),
                    ms(t0, 30),
                    &RULES,
                    video(Stage::Running),
                    &expected,
                    Next::Sleep(INTERVAL - Duration::from_millis(30)),
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
            (UiSupport::Reopenable, true),
            (UiSupport::Works, false),
        ] {
            let mut actions = Vec::new();
            if report {
                actions.push(reported.clone());
            }
            actions.push(Action::Deliver(frame.clone()));
            let state = check(
                at(t0, Phase::Ui { held: true }, ui),
                Event::UiAcquired(Ok(frame.clone())),
                t0,
                &RULES,
                Phase::Ui { held: true },
                &actions,
                Next::Sleep(INTERVAL),
            );
            assert_eq!(state.ui, UiSupport::Works);
        }
    }

    /// A pending driver is timed from its first answer without a frame; one
    /// that stays pending for `pending` is released and a new one opened at
    /// once. A second stall releases the tap and reports the observed stall.
    #[test]
    fn a_pending_driver_stalls_after_its_limit() {
        let t0 = Instant::now();
        let state = check(
            at(t0, video(Stage::Running), UiSupport::Untried),
            Event::Acquired(Ok(Acquired::Pending)),
            t0,
            &RULES,
            video(no_frame_pending(t0, t0)),
            &[],
            Next::Sleep(INTERVAL),
        );
        let state = check(
            state,
            Event::Acquired(Ok(Acquired::Pending)),
            ms(t0, 1999),
            &RULES,
            video(no_frame_pending(t0, ms(t0, 1999))),
            &[],
            Next::Sleep(Duration::ZERO),
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
            Event::Acquired(Ok(Acquired::Pending)),
            first,
            &RULES,
            period(Stage::Closed, NewDriver::ReopenedAfterStall, None),
            &[
                note(Note::VideoRetrying { error: stalled() }),
                Action::Release,
            ],
            Next::Open(Kind::Video),
        );
        let state = next(state, Event::Opened, first, &RULES).state;
        let state = next(state, Event::Acquired(Ok(Acquired::Pending)), first, &RULES).state;
        let second = ms(t0, 4000);
        check(
            state,
            Event::Acquired(Ok(Acquired::Pending)),
            second,
            &RULES,
            Phase::Idle,
            &[Action::Release, Action::ReportStall(stalled())],
            Next::End,
        );
    }

    /// A driver not ready is asked again after `not_ready_retry`, and fails the source
    /// once it stayed not ready for `preparation`, its tap released after.
    #[test]
    fn a_preparing_driver_fails_after_its_limit() {
        let t0 = Instant::now();
        let preparing = |at_ms| video(no_frame_not_ready(t0, ms(t0, at_ms)));
        let state = check(
            at(t0, video(Stage::Created), UiSupport::Untried),
            Event::Acquired(Ok(Acquired::NotReady)),
            t0,
            &RULES,
            preparing(1000),
            &[note(Note::InitializationAnswered {
                answer: Answer::NotReady,
                since_change_ms: 0,
            })],
            Next::Sleep(INTERVAL),
        );
        let state = check(
            state,
            Event::Woke,
            ms(t0, 400),
            &RULES,
            preparing(1000),
            &[],
            Next::Sleep(Duration::from_millis(600)),
        );
        let state = check(
            state,
            Event::Acquired(Ok(Acquired::NotReady)),
            ms(t0, 9999),
            &RULES,
            preparing(10_999),
            &[note(Note::InitializationAnswered {
                answer: Answer::NotReady,
                since_change_ms: 9999,
            })],
            Next::Sleep(Duration::ZERO),
        );
        let failed = Error::other(format!(
            "webOS video was not ready within {:?}",
            LIMITS.preparation
        ));
        check(
            state,
            Event::Acquired(Ok(Acquired::NotReady)),
            ms(t0, 10_000),
            &RULES,
            Phase::Idle,
            &[
                note(Note::InitializationAnswered {
                    answer: Answer::NotReady,
                    since_change_ms: 10_000,
                }),
                Action::Fail(failed),
                Action::Release,
            ],
            Next::End,
        );
    }

    /// Two clocks: `preparation` counts from the first answer without a
    /// frame of either kind, so alternating answers reach it and fail the
    /// source; `pending` counts from the first of a run of pending answers
    /// (the driver was initialized then), so a slow preparation does not
    /// shorten it.
    #[test]
    fn a_stall_counts_from_its_first_pending_answer() {
        let t0 = Instant::now();
        let preparing = |at_ms| video(no_frame_not_ready(t0, ms(t0, at_ms)));
        let pending = |since_ms, at_ms| {
            video(Stage::NoFrame {
                since: t0,
                pending_since: Some(ms(t0, since_ms)),
                at: ms(t0, at_ms),
            })
        };
        let not_ready: fn() -> Acquired = || Acquired::NotReady;
        let buffer: fn() -> Acquired = || Acquired::Pending;
        let paths = [
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
            vec![
                (0, not_ready, preparing(1000)),
                (1000, not_ready, preparing(2000)),
                (2000, not_ready, preparing(3000)),
                (3000, not_ready, preparing(4000)),
                (3100, buffer, pending(3100, 3100)),
                (5099, buffer, pending(3100, 5099)),
                (
                    5100,
                    buffer,
                    period(Stage::Closed, NewDriver::ReopenedAfterStall, None),
                ),
            ],
        ];
        for path in paths {
            let mut state = at(t0, video(Stage::Running), UiSupport::Untried);
            for (now, answer, phase) in path {
                state = next(state, Event::Acquired(Ok(answer())), ms(t0, now), &RULES).state;
                assert_eq!(state.phase, phase, "at {now} ms");
            }
        }
    }

    /// A change of the video context initializes a driver not ready again
    /// at once, without waiting out `not_ready_retry`, and the preparation
    /// limit counts again from its next answer; a signal that changes only
    /// the sequence waits it out and keeps the count.
    #[test]
    fn a_context_change_restarts_the_preparation_limit() {
        let t0 = Instant::now();
        let preparing =
            |since_ms, at_ms| video(no_frame_not_ready(ms(t0, since_ms), ms(t0, at_ms)));
        let not_ready = || Event::Acquired(Ok(Acquired::NotReady));
        let reset = |sequence| Event::Signal(ready(Video::Available, 1, sequence));
        let same = Event::Signal(ready(Video::Available, 0, 2));
        let path = [
            (0, not_ready(), preparing(0, 1000), Next::Sleep(INTERVAL)),
            (
                500,
                same,
                preparing(0, 1000),
                Next::Sleep(Duration::from_millis(500)),
            ),
            (9000, reset(3), video(Stage::Created), Next::Acquire),
            (
                9000,
                not_ready(),
                preparing(9000, 10_000),
                Next::Sleep(Duration::ZERO),
            ),
            (
                10_000,
                not_ready(),
                preparing(9000, 11_000),
                Next::Sleep(Duration::ZERO),
            ),
            (
                18_999,
                not_ready(),
                preparing(9000, 19_999),
                Next::Sleep(Duration::ZERO),
            ),
            (19_000, not_ready(), Phase::Idle, Next::End),
        ];
        let mut state = at(t0, video(Stage::Running), UiSupport::Untried);
        for (now, event, phase, operation) in path {
            let step = next(state, event, ms(t0, now), &RULES);
            assert_eq!(
                (step.state.phase, step.next),
                (phase, operation),
                "at {now} ms"
            );
            state = step.state;
        }
    }

    /// The pace skips missed ticks instead of bursting.
    #[test]
    fn the_pace_skips_missed_ticks() {
        let t0 = Instant::now();
        let late = ms(t0, 950);
        let state = check(
            at(t0, video(Stage::Running), UiSupport::Untried),
            Event::Acquired(Ok(Acquired::Frame(frame()))),
            late,
            &RULES,
            video(Stage::Running),
            &[Action::Deliver(frame())],
            Next::Sleep(Duration::ZERO),
        );
        assert_eq!(state.tick, late);
    }

    /// A refusal tells the pulls once and waits for any change of the
    /// signal, which initializes the driver once more; a second refusal
    /// counts on, as a repeat. A not-ready answer leaves the pulls refused;
    /// only the frame tells them, once.
    #[test]
    fn a_refusal_waits_for_a_change_and_lasts_until_a_frame() {
        let t0 = Instant::now();
        let since = Some(ms(t0, 100));
        let refusing = |stage| period(stage, NewDriver::Available, since);
        let answered = |answer| {
            note(Note::InitializationAnswered {
                answer,
                since_change_ms: 100,
            })
        };
        let state = check(
            at(t0, video(Stage::Created), UiSupport::Untried),
            Event::Acquired(Ok(Acquired::Refused)),
            ms(t0, 100),
            &RULES,
            refusing(refused(1)),
            &[
                answered(Answer::Refused),
                Action::TellRefused(true),
                note(Note::Refused {
                    initializations: 1,
                    since_change_ms: 100,
                    occurrence: Occurrence::First,
                }),
            ],
            Next::Sleep(Duration::ZERO),
        );
        let state = check(
            state,
            Event::Woke,
            ms(t0, 200),
            &RULES,
            refusing(refused(1)),
            &[],
            Next::Idle,
        );
        let same = ready(Video::Available, 0, 2);
        let state = check(
            state,
            Event::Signal(same),
            ms(t0, 300),
            &RULES,
            refusing(refused(1)),
            &[
                note(Note::SignalChanged(same)),
                note(Note::ChangedWhileRefused {
                    video: Video::Available,
                    generation: 0,
                }),
            ],
            Next::Acquire,
        );
        let state = check(
            state,
            Event::Acquired(Ok(Acquired::Refused)),
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
            ],
            Next::Sleep(Duration::ZERO),
        );
        let changed = ready(Video::Available, 0, 3);
        let state = next(state, Event::Signal(changed), ms(t0, 500), &RULES).state;
        let state = check(
            state,
            Event::Acquired(Ok(Acquired::NotReady)),
            ms(t0, 600),
            &RULES,
            refusing(no_frame_not_ready(ms(t0, 600), ms(t0, 1600))),
            &[answered(Answer::NotReady)],
            Next::Sleep(Duration::ZERO),
        );
        check(
            state,
            Event::Acquired(Ok(Acquired::Frame(frame()))),
            ms(t0, 1600),
            &RULES,
            video(Stage::Running),
            &[
                note(Note::InitializationAnswered {
                    answer: Answer::Accepted,
                    since_change_ms: 1100,
                }),
                Action::Initialized,
                Action::TellRefused(false),
                note(Note::RefusalEnded {
                    by: RefusalEnd::Frame,
                    elapsed_ms: 1500,
                }),
                Action::Deliver(frame()),
            ],
            Next::Sleep(Duration::ZERO),
        );
    }

    /// The pulls' refused state outlives every answer but a frame: a
    /// not-ready or pending answer and a fault that opens a new driver
    /// leave it; a frame ends it, as does the source's failure.
    #[test]
    fn only_a_frame_or_a_failure_ends_the_refused_state() {
        let t0 = Instant::now();
        let fresh = fault_of(Recovery::Retry(Reason::FreshDriver));
        use NewDriver::{Available, Spent};
        for (answer, new_driver, told, still) in [
            (Ok(Acquired::NotReady), Available, None, true),
            (Ok(Acquired::Pending), Available, None, true),
            (Err(fresh), Available, None, true),
            (Ok(Acquired::Frame(frame())), Available, Some(false), false),
            (Err(fault()), Spent, Some(false), false),
        ] {
            let state = at(
                t0,
                period(refused(1), new_driver, Some(t0)),
                UiSupport::Untried,
            );
            let step = next(state, Event::Acquired(answer), ms(t0, 250), &RULES);
            let refused: Vec<_> = step
                .actions
                .iter()
                .filter_map(|action| match action {
                    Action::TellRefused(refused) => Some(*refused),
                    _ => None,
                })
                .collect();
            assert_eq!(refused, Vec::from_iter(told), "{:?}", step.actions);
            let kept = matches!(
                step.state.phase,
                Phase::Video(VideoPeriod {
                    refused_since: Some(_),
                    ..
                })
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
        let open = Next::Open(Kind::Video);
        let released = || vec![Action::Release];
        for (stage, phase, actions, operation) in [
            (refused(1), video(refused(1)), vec![], Next::Acquire),
            (
                no_frame_not_ready(t0, ms(t0, 900)),
                video(Stage::Created),
                vec![],
                Next::Acquire,
            ),
            (no_frame_pending(t0, t0), closed, released(), open),
            (Stage::Running, closed, released(), open),
            (Stage::Closed, closed, vec![], open),
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
                at(t0, spent(stage, None), UiSupport::Untried),
                Event::Signal(reset),
                now,
                &RULES,
                phase,
                &expected,
                operation,
            );
        }
    }

    /// Video becoming unavailable releases a refused driver and ends the
    /// pulls' refused state; it does not note initializing the driver
    /// again, which it does not do.
    #[test]
    fn video_leaving_ends_a_refusal() {
        let t0 = Instant::now();
        let gone = ready(Video::Unavailable, 0, 2);
        for (rules, phase, operation) in [
            (NO_UI, Phase::Idle, Next::Idle),
            (RULES, Phase::Ui { held: false }, Next::Open(Kind::Ui)),
        ] {
            let actions = [
                note(Note::SignalChanged(gone)),
                note(Note::VideoChanged(Video::Unavailable)),
                Action::Release,
                Action::TellRefused(false),
                note(Note::RefusalEnded {
                    by: RefusalEnd::KindChange,
                    elapsed_ms: 700,
                }),
            ];
            check(
                at(
                    t0,
                    period(refused(3), NewDriver::Available, Some(t0)),
                    UiSupport::Untried,
                ),
                Event::Signal(gone),
                ms(t0, 700),
                &rules,
                phase,
                &actions,
                operation,
            );
        }
    }

    /// Every video fault in every held stage, with its one new driver
    /// unspent or spent (a fault since the last frame): a retry releases
    /// the tap and opens a new one at once, one time; given up, an ordinary
    /// fault fails the source and a second stall reports it. Withheld video releases the tap,
    /// tells the pulls refused and idles; an escalation fails the source.
    #[test]
    fn every_video_fault_in_each_held_stage() {
        use NewDriver::{Available, ReopenedAfterStall, Spent};
        let t0 = Instant::now();
        let error = || Error::other("interrupted");
        let fresh = Recovery::Retry(Reason::FreshDriver);
        let stalled = Recovery::Retry(Reason::StalledDriver);
        let escalate = Recovery::Escalate(prysm_ports::Escalation::Restart);
        let retried = || {
            (
                vec![
                    note(Note::VideoRetrying { error: error() }),
                    Action::Release,
                ],
                Next::Open(Kind::Video),
            )
        };
        let failed = || (vec![Action::Fail(error()), Action::Release], Next::End);
        let withheld = || {
            (
                vec![
                    Action::Release,
                    Action::TellRefused(true),
                    note(Note::Withheld {
                        error: error(),
                        occurrence: Occurrence::First,
                    }),
                ],
                Next::Idle,
            )
        };
        let initializing = note(Note::InitializationAnswered {
            answer: Answer::Failed,
            since_change_ms: 0,
        });
        for (stage, answered) in [
            (Stage::Created, vec![initializing]),
            (Stage::Running, vec![]),
            (no_frame_pending(t0, t0), vec![]),
        ] {
            for (recovery, new_driver, phase, (tail, operation)) in [
                (fresh, Available, spent(Stage::Closed, None), retried()),
                (fresh, Spent, Phase::Idle, failed()),
                (
                    stalled,
                    Available,
                    period(Stage::Closed, ReopenedAfterStall, None),
                    retried(),
                ),
                (
                    stalled,
                    Spent,
                    period(Stage::Closed, ReopenedAfterStall, None),
                    retried(),
                ),
                (
                    stalled,
                    ReopenedAfterStall,
                    Phase::Idle,
                    (
                        vec![Action::Release, Action::ReportStall(error())],
                        Next::End,
                    ),
                ),
                (
                    Recovery::Withheld,
                    Available,
                    period(Stage::Withheld, Available, Some(t0)),
                    withheld(),
                ),
                (
                    Recovery::Withheld,
                    Spent,
                    spent(Stage::Withheld, Some(t0)),
                    withheld(),
                ),
                (escalate, Available, Phase::Idle, failed()),
            ] {
                check(
                    at(t0, period(stage, new_driver, None), UiSupport::Untried),
                    Event::Acquired(Err(fault_of(recovery))),
                    t0,
                    &RULES,
                    phase,
                    &[answered.clone(), tail].concat(),
                    operation,
                );
            }
        }
    }

    /// While the pulls are told refused on a new tap, only a frame or the
    /// source's failure ends it: a first retry, a not-ready and a pending
    /// answer leave the pulls refused; a failure tells the end before it
    /// fails.
    #[test]
    fn a_withholding_outlives_what_is_not_a_frame() {
        use NewDriver::{Available, Spent};
        let t0 = Instant::now();
        let error = || Error::other("interrupted");
        let fresh = Recovery::Retry(Reason::FreshDriver);
        let escalate = Recovery::Escalate(prysm_ports::Escalation::FAIL_FAST);
        let failed = || {
            vec![
                Action::TellRefused(false),
                Action::Fail(error()),
                Action::Release,
            ]
        };
        for (new_driver, answer, phase, expected, operation) in [
            (
                Available,
                Err(fault_of(fresh)),
                spent(Stage::Closed, Some(t0)),
                vec![
                    note(Note::VideoRetrying { error: error() }),
                    Action::Release,
                ],
                Next::Open(Kind::Video),
            ),
            (
                Spent,
                Err(fault_of(fresh)),
                Phase::Idle,
                failed(),
                Next::End,
            ),
            (
                Available,
                Err(fault_of(escalate)),
                Phase::Idle,
                failed(),
                Next::End,
            ),
            (
                Available,
                Ok(Acquired::Pending),
                period(no_frame_pending(t0, t0), Available, Some(t0)),
                vec![],
                Next::Sleep(INTERVAL),
            ),
            (
                Available,
                Ok(Acquired::NotReady),
                period(
                    no_frame_not_ready(t0, t0 + LIMITS.not_ready_retry),
                    Available,
                    Some(t0),
                ),
                vec![],
                Next::Sleep(INTERVAL),
            ),
        ] {
            check(
                at(
                    t0,
                    period(Stage::Running, new_driver, Some(t0)),
                    UiSupport::Untried,
                ),
                Event::Acquired(answer),
                t0,
                &RULES,
                phase,
                &expected,
                operation,
            );
        }
    }

    /// An open fault holds no tap, so none is released.
    #[test]
    fn every_open_fault() {
        use NewDriver::{Available, Spent};
        let t0 = Instant::now();
        let error = || Error::other("interrupted");
        let fresh = Recovery::Retry(Reason::FreshDriver);
        let failed = || vec![Action::Fail(error())];
        for (recovery, new_driver, phase, expected, operation) in [
            (
                fresh,
                Available,
                spent(Stage::Closed, None),
                vec![note(Note::VideoRetrying { error: error() })],
                Next::Open(Kind::Video),
            ),
            (fresh, Spent, Phase::Idle, failed(), Next::End),
            (
                Recovery::Withheld,
                Available,
                period(Stage::Withheld, Available, Some(t0)),
                vec![
                    Action::TellRefused(true),
                    note(Note::Withheld {
                        error: error(),
                        occurrence: Occurrence::First,
                    }),
                ],
                Next::Idle,
            ),
            (
                Recovery::Escalate(prysm_ports::Escalation::FAIL_FAST),
                Available,
                Phase::Idle,
                failed(),
                Next::End,
            ),
        ] {
            check(
                at(
                    t0,
                    period(Stage::Closed, new_driver, None),
                    UiSupport::Untried,
                ),
                Event::OpenFailed(fault_of(recovery)),
                t0,
                &RULES,
                phase,
                &expected,
                operation,
            );
        }
    }

    /// Withheld video idles until any change of the video signal, which
    /// opens a new driver at once and keeps the refused state: the
    /// pulls stay refused, and withheld again, it is not told again.
    #[test]
    fn withheld_video_ends_its_idle_at_a_signal_change() {
        let t0 = Instant::now();
        let withheld = period(Stage::Withheld, NewDriver::Available, Some(t0));
        let state = check(
            at(t0, withheld, UiSupport::Untried),
            Event::Woke,
            ms(t0, 100),
            &RULES,
            withheld,
            &[],
            Next::Idle,
        );
        let same = ready(Video::Available, 0, 2);
        let reopened = period(Stage::Closed, NewDriver::Available, Some(t0));
        let state = check(
            state,
            Event::Signal(same),
            ms(t0, 200),
            &RULES,
            reopened,
            &[
                note(Note::SignalChanged(same)),
                note(Note::ChangedWhileWithheld {
                    video: Video::Available,
                    generation: 0,
                }),
            ],
            Next::Open(Kind::Video),
        );
        let reset = ready(Video::Available, 1, 2);
        check(
            at(t0, withheld, UiSupport::Untried),
            Event::Signal(reset),
            ms(t0, 200),
            &RULES,
            reopened,
            &[
                note(Note::SignalChanged(reset)),
                note(Note::ChangedWhileWithheld {
                    video: Video::Available,
                    generation: 1,
                }),
            ],
            Next::Open(Kind::Video),
        );
        let state = next(state, Event::Opened, ms(t0, 200), &RULES).state;
        check(
            state,
            Event::Acquired(Err(fault_of(Recovery::Withheld))),
            ms(t0, 300),
            &RULES,
            withheld,
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
            ],
            Next::Idle,
        );
    }

    /// A change of tap kind ends the refused state: the pulls are told
    /// once, no new driver is noted, and nothing is released while
    /// withheld video holds no tap. A stop while withheld releases nothing
    /// either.
    #[test]
    fn a_kind_change_or_a_stop_ends_a_withholding_without_a_release() {
        let t0 = Instant::now();
        let gone = ready(Video::Unavailable, 0, 2);
        for (stage, release) in [(Stage::Withheld, false), (Stage::Running, true)] {
            let mut expected = vec![
                note(Note::SignalChanged(gone)),
                note(Note::VideoChanged(Video::Unavailable)),
            ];
            if release {
                expected.push(Action::Release);
            }
            expected.extend([
                Action::TellRefused(false),
                note(Note::RefusalEnded {
                    by: RefusalEnd::KindChange,
                    elapsed_ms: 400,
                }),
            ]);
            check(
                at(t0, spent(stage, Some(t0)), UiSupport::Untried),
                Event::Signal(gone),
                ms(t0, 400),
                &NO_UI,
                Phase::Idle,
                &expected,
                Next::Idle,
            );
        }
        check(
            at(t0, spent(Stage::Withheld, Some(t0)), UiSupport::Untried),
            Event::Stop,
            t0,
            &RULES,
            Phase::Idle,
            &[],
            Next::End,
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
            state = next(state, Event::Acquired(Err(fresh())), t0, &RULES).state;
            assert_eq!(state.phase, spent(Stage::Closed, None));
            state = next(state, Event::Opened, t0, &RULES).state;
            state = next(
                state,
                Event::Acquired(Ok(Acquired::Frame(frame()))),
                t0,
                &RULES,
            )
            .state;
            assert_eq!(state.phase, video(Stage::Running));
        }
        let step = next(state, Event::Acquired(Err(fresh())), t0, &RULES);
        assert_eq!(step.state.phase, spent(Stage::Closed, None));
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
                Action::TellRefused(refused) => Some(*refused),
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
        let change = |sequence| Event::Signal(ready(Video::Available, 0, sequence));
        let (state, actions) = run(
            t0,
            at(t0, video(Stage::Running), UiSupport::Untried),
            vec![
                (0, Event::Acquired(Err(fault_of(Recovery::Withheld)))),
                (100, change(2)),
                (100, Event::Opened),
                (200, Event::Acquired(Ok(Acquired::Refused))),
                (300, change(3)),
                (300, Event::Acquired(Ok(Acquired::NotReady))),
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
            Phase::Video(VideoPeriod {
                stage: Stage::NoFrame { .. },
                refused_since: Some(since),
                ..
            }) if since == t0
        ));
        let (state, actions) = run(
            t0,
            state,
            vec![(400, Event::Acquired(Ok(Acquired::Frame(frame()))))],
            &RULES,
        );
        assert_eq!(refused_told(&actions), [false]);
        assert!(matches!(
            state.phase,
            Phase::Video(VideoPeriod {
                refused_since: None,
                ..
            })
        ));
    }

    #[test]
    fn a_signal_change_gives_stall_recovery_back() {
        let t0 = Instant::now();
        let (state, _) = run(
            t0,
            at(t0, video(Stage::Running), UiSupport::Untried),
            vec![
                (0, Event::Acquired(Ok(Acquired::Pending))),
                (2000, Event::Acquired(Ok(Acquired::Pending))),
                (2000, Event::Opened),
                (2001, Event::Signal(ready(Video::Available, 0, 2))),
                (2001, Event::Acquired(Ok(Acquired::Pending))),
            ],
            &RULES,
        );
        let stall = next(
            state,
            Event::Acquired(Ok(Acquired::Pending)),
            ms(t0, 4001),
            &RULES,
        );
        assert_eq!(stall.next, Next::Open(Kind::Video));
        assert!(refused_told(&stall.actions).is_empty());
    }

    #[test]
    fn accepted_pending_video_and_only_successful_open_of_ui_prove_initialization() {
        let t0 = Instant::now();
        for (answer, initialized) in [
            (Acquired::Pending, true),
            (Acquired::Frame(frame()), true),
            (Acquired::NotReady, false),
            (Acquired::Refused, false),
        ] {
            let seen = next(
                at(t0, video(Stage::Created), UiSupport::Untried),
                Event::Acquired(Ok(answer)),
                t0,
                &RULES,
            );
            assert_eq!(
                seen.actions
                    .iter()
                    .any(|action| matches!(action, Action::Initialized)),
                initialized
            );
        }
        let ui = next(
            at(t0, Phase::Ui { held: false }, UiSupport::Untried),
            Event::Opened,
            t0,
            &RULES,
        );
        assert!(
            ui.actions
                .iter()
                .any(|action| matches!(action, Action::Initialized))
        );
    }

    /// A stop releases the tap iff one is held.
    #[test]
    fn a_stop_releases_only_a_held_tap() {
        let t0 = Instant::now();
        let closed = Stage::Closed;
        let held = [
            Stage::Running,
            no_frame_pending(t0, t0),
            refused(1),
            Stage::Created,
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
                Next::End,
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
            note(Note::UiFault {
                recovery: UiRecovery::MayPass,
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
            &[ui_failed(Occurrence::First), Action::Release],
            Next::Idle,
        );
        assert_eq!(state.ui, UiSupport::Failed);
        let back = ready(Video::Available, 0, 2);
        let state = next(state, Event::Signal(back), ms(t0, 200), &RULES).state;
        assert_eq!(state.ui, UiSupport::Reopenable);
        let state = next(state, Event::Opened, ms(t0, 200), &RULES).state;
        let gone = ready(Video::Unavailable, 0, 3);
        let state = check(
            state,
            Event::Signal(gone),
            ms(t0, 300),
            &RULES,
            Phase::Ui { held: false },
            &[
                note(Note::SignalChanged(gone)),
                note(Note::VideoChanged(Video::Unavailable)),
                Action::Release,
            ],
            Next::Open(Kind::Ui),
        );
        let state = check(
            state,
            Event::UiOpenFailed(ui_fault(UiRecovery::MayPass)),
            ms(t0, 300),
            &RULES,
            Phase::Idle,
            &[ui_failed(Occurrence::Again)],
            Next::Idle,
        );
        assert_eq!(state.ui, UiSupport::Failed);
    }

    /// A UI fault ends the UI period with no timer: a wake, or a signal
    /// that changes neither the video nor its context, only idles; a context
    /// reset revives the UI and opens it at once.
    #[test]
    fn a_failed_ui_waits_for_the_video_or_its_context() {
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
            &[],
            Next::Idle,
        );
        let reset = ready(Video::Unavailable, 1, 2);
        check(
            state,
            Event::Signal(reset),
            ms(t0, 100),
            &RULES,
            Phase::Ui { held: false },
            &[note(Note::SignalChanged(reset))],
            Next::Open(Kind::Ui),
        );
        let same = ready(Video::Unavailable, 0, 2);
        check(
            state,
            Event::Signal(same),
            ms(t0, 100),
            &RULES,
            Phase::Idle,
            &[note(Note::SignalChanged(same))],
            Next::Idle,
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
                [Note::UiFault {
                    recovery: UiRecovery::Unsupported,
                    error,
                    occurrence: Occurrence::First
                }]
            )
        );
        let mut state = state.state;
        for (sequence, video) in [(2, Video::Available), (3, Video::Unavailable)] {
            state = next(state, Event::Signal(ready(video, 1, sequence)), t0, &RULES).state;
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
        let apart = next(running, Event::Signal(gone), t0, &NO_UI).state;
        check(
            apart,
            Event::Signal(back),
            t0,
            &NO_UI,
            video(Stage::Closed),
            &[
                note(Note::SignalChanged(back)),
                note(Note::VideoChanged(Video::Available)),
            ],
            Next::Open(Kind::Video),
        );
        check(
            running,
            Event::Signal(back),
            t0,
            &NO_UI,
            video(Stage::Running),
            &[note(Note::SignalChanged(back))],
            Next::Acquire,
        );
        let refused = at(
            t0,
            period(refused(1), NewDriver::Available, Some(t0)),
            UiSupport::Untried,
        );
        let once = next(
            refused,
            Event::Signal(ready(Video::Available, 0, 3)),
            t0,
            &RULES,
        );
        let first = next(
            refused,
            Event::Signal(ready(Video::Available, 0, 2)),
            t0,
            &RULES,
        );
        let twice = next(
            first.state,
            Event::Signal(ready(Video::Available, 0, 3)),
            t0,
            &RULES,
        );
        assert_eq!(once.state, twice.state);
        assert_eq!(
            format!("{:?}", &once.actions[1..]),
            format!("{:?}", &twice.actions[1..])
        );
        assert_eq!(once.next, twice.next);
    }

    #[test]
    fn the_tap_follows_the_video_the_setting_and_the_ui_state() {
        use UiSupport::{Failed, Reopenable, Unsupported, Untried, Works};
        let ui_tap = Some(Kind::Ui);
        for (ui, on, off) in [
            (Untried, ui_tap, None),
            (Works, ui_tap, None),
            (Failed, None, None),
            (Reopenable, ui_tap, None),
            (Unsupported, None, None),
        ] {
            for include_ui in [false, true] {
                for video in [Video::Available, Video::Unknown] {
                    assert_eq!(wanted(video, include_ui, ui), Some(Kind::Video));
                }
            }
            assert_eq!(wanted(Video::Unavailable, true, ui), on, "{ui:?}");
            assert_eq!(wanted(Video::Unavailable, false, ui), off, "{ui:?}");
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
            let mut actions = vec![note(Note::UiFault {
                recovery,
                error,
                occurrence,
            })];
            if held {
                actions.push(Action::Release);
            }
            let state = at(t0, Phase::Ui { held }, UiSupport::Works);
            let after = check(state, event, t0, &RULES, Phase::Idle, &actions, Next::Idle);
            assert_eq!(after.ui, ui, "{held}");
        }
    }

    /// A context reset reopens a held TV UI tap and leaves an idle source
    /// idle.
    #[test]
    fn a_context_reset_without_video() {
        let t0 = Instant::now();
        let reset = ready(Video::Unavailable, 1, 2);
        for (phase, rules, after, actions, operation) in [
            (
                Phase::Ui { held: true },
                RULES,
                Phase::Ui { held: false },
                vec![Action::Release],
                Next::Open(Kind::Ui),
            ),
            (Phase::Idle, NO_UI, Phase::Idle, vec![], Next::Idle),
        ] {
            let mut state = at(t0, phase, UiSupport::Works);
            state.seen = Some(Seen {
                readiness: ready(Video::Unavailable, 0, 1),
                at: t0,
            });
            let mut expected = vec![note(Note::SignalChanged(reset))];
            expected.extend(actions);
            check(
                state,
                Event::Signal(reset),
                t0,
                &rules,
                after,
                &expected,
                operation,
            );
        }
    }
}
