use super::*;
use crate::transport::{Mode, Wire, WriteError};
use futures::StreamExt;
use prysm_contract::{
    identity::{FrameStamp, SettingsRevision},
    observation::Update,
};
use prysm_core::{ColorStrip, LinearColor};
use std::{
    num::{NonZeroU64, NonZeroUsize},
    time::Duration,
};
use tokio::task::JoinHandle;

mod network;

const INSTANCE: InstanceId = InstanceId(NonZeroU64::MIN);
fn session(sequence: u64) -> SessionId {
    SessionId {
        instance: INSTANCE,
        sequence: NonZeroU64::new(sequence).unwrap(),
    }
}
fn epoch(generation: u64) -> StreamEpoch {
    StreamEpoch {
        session: session(1),
        generation,
    }
}
fn settings() -> WledSettings {
    WledSettings {
        address: "127.0.0.1".parse().unwrap(),
        leds: [1, 0, 0, 0],
    }
}
fn red() -> EdgeColors {
    EdgeColors {
        top: ColorStrip::fill(LinearColor::new(1.0, 0.0, 0.0), NonZeroUsize::MIN),
        ..Default::default()
    }
}
fn frame(generation: u64, sequence: u64) -> ColorFrame {
    ColorFrame {
        stamp: FrameStamp {
            epoch: epoch(generation),
            sequence,
        },
        revision: SettingsRevision(0),
        colors: red(),
    }
}
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("output made no progress")
}

#[derive(Debug, PartialEq)]
enum Kind {
    Configure,
    Mode(Mode),
    Colors(EdgeColors),
}
struct Call {
    kind: Kind,
    reply: Option<oneshot::Sender<Result<(), WriteError>>>,
}
impl Call {
    fn ok(self) {
        self.reply
            .expect("async call")
            .send(Ok(()))
            .ok()
            .expect("write remained owned");
    }
    fn fail(self, uncertain: bool) {
        self.reply
            .expect("async call")
            .send(Err(WriteError {
                error: ControlError::Unavailable("test write failed"),
                uncertain,
            }))
            .ok()
            .expect("write remained owned");
    }
}
struct GatedWire(mpsc::UnboundedSender<Call>);
impl Wire for GatedWire {
    fn configure(&mut self, _: &WledSettings) -> Result<(), ControlError> {
        self.0
            .send(Call {
                kind: Kind::Configure,
                reply: None,
            })
            .unwrap();
        Ok(())
    }
    async fn mode(&mut self, mode: Mode) -> Result<(), WriteError> {
        if mode == Mode::Release {
            return Ok(());
        }
        let (reply, result) = oneshot::channel();
        self.0
            .send(Call {
                kind: Kind::Mode(mode),
                reply: Some(reply),
            })
            .unwrap();
        result.await.unwrap()
    }
    async fn colors(&mut self, colors: &EdgeColors) -> Result<(), ControlError> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Call {
                kind: Kind::Colors(colors.clone()),
                reply: Some(reply),
            })
            .unwrap();
        result.await.unwrap().map_err(|error| error.error)
    }
}
struct Running {
    control: WledHandle,
    colors: WledColors,
    terminal: WledShutdown,
    calls: mpsc::UnboundedReceiver<Call>,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<OwnerStopped, ControlError>>,
}
impl Running {
    async fn new() -> Self {
        let output = WledOutput::new(INSTANCE);
        let (sender, calls) = mpsc::unbounded_channel();
        let (shutdown, stopped) = oneshot::channel();
        let task = tokio::spawn(owner::run(output.owner, GatedWire(sender), async {
            let _ = stopped.await;
        }));
        let mut running = Self {
            control: output.control,
            colors: output.colors,
            terminal: output.shutdown,
            calls,
            shutdown: Some(shutdown),
            task,
        };
        assert_eq!(
            within(running.control.configure(settings())).await.unwrap(),
            OutputApplied::Sent
        );
        running.call(Kind::Configure).await;
        running
    }
    async fn call(&mut self, expected: Kind) -> Call {
        let call = within(self.calls.recv()).await.unwrap();
        assert_eq!(call.kind, expected);
        call
    }
    fn hold_request(&self, generation: u64) -> JoinHandle<Result<OutputApplied, ControlError>> {
        let control = self.control.clone();
        tokio::spawn(async move { control.hold(epoch(generation), EdgeColors::default()).await })
    }
    fn finish_request(
        &self,
        mode: IdleOutputMode,
    ) -> JoinHandle<Result<OutputApplied, ControlError>> {
        let control = self.control.clone();
        tokio::spawn(async move { control.finish(session(1), mode).await })
    }
    async fn hold(&mut self) {
        let request = self.hold_request(0);
        self.call(Kind::Mode(Mode::Realtime)).await.ok();
        self.call(Kind::Colors(EdgeColors::default())).await.ok();
        assert_eq!(within(request).await.unwrap().unwrap(), OutputApplied::Sent);
    }
    async fn admit(&mut self, generation: u64, colors: EdgeColors) {
        let control = self.control.clone();
        let request = tokio::spawn(async move { control.admit(epoch(generation)).await });
        self.call(Kind::Colors(colors)).await.ok();
        assert_eq!(within(request).await.unwrap().unwrap(), OutputApplied::Sent);
    }
    async fn off(&mut self) {
        let request = self.finish_request(IdleOutputMode::Off);
        self.call(Kind::Mode(Mode::Off)).await.ok();
        within(request).await.unwrap().unwrap();
    }
    async fn close(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.send(()).unwrap();
        }
        assert_eq!(within(self.task).await.unwrap().unwrap(), OwnerStopped);
    }
}

#[test]
fn validation_rejects_invalid_addresses_and_led_totals() {
    let mut config = settings();
    for leds in [[0; 4], [u32::MAX; 4], [65536, 0, 0, 0]] {
        config.leds = leds;
        assert!(matches!(
            config.validate(),
            Err(ControlError::Invalid { field: "leds", .. })
        ));
    }
    config.leds = [65535, 0, 0, 0];
    config.validate().unwrap();
    for address in [
        "0.0.0.0",
        "255.255.255.255",
        "239.1.2.3",
        "::",
        "ff02::1",
        "::ffff:239.1.2.3",
    ] {
        config.address = address.parse().unwrap();
        assert!(matches!(
            config.validate(),
            Err(ControlError::Invalid {
                field: "address",
                ..
            })
        ));
    }
}

#[test]
fn admission_retains_one_newest_future_candidate_and_keeps_its_watermark() {
    let mut gate = prysm_output::Admission::new(INSTANCE);
    gate.hold(epoch(0)).unwrap();
    assert_eq!(gate.offer(frame(2, 2)), OfferResult::Queued);
    assert_eq!(gate.offer(frame(1, 100)), OfferResult::DiscardedStale);
    assert_eq!(gate.offer(frame(2, 1)), OfferResult::DiscardedStale);
    gate.admit(epoch(1)).unwrap();
    assert!(gate.take().is_none());
    gate.admit(epoch(2)).unwrap();
    assert_eq!(gate.take().unwrap().stamp, frame(2, 2).stamp);
    assert!(gate.take().is_none());
    assert_eq!(gate.offer(frame(2, 2)), OfferResult::DiscardedStale);
    gate.hold(epoch(2)).unwrap();
    assert!(gate.admit(epoch(2)).is_err());
    assert_eq!(gate.offer(frame(2, 99)), OfferResult::DiscardedStale);
    assert_eq!(gate.offer(frame(3, 1)), OfferResult::Queued);
    assert!(gate.take().is_none());
    gate.finish(session(1)).unwrap();
    assert_eq!(gate.offer(frame(3, 2)), OfferResult::DiscardedStale);
    assert!(gate.hold(epoch(3)).is_err());
    assert!(gate.can_configure().is_err());
    gate.finished(session(1));
    let next = StreamEpoch {
        session: session(2),
        generation: 0,
    };
    gate.hold(next).unwrap();
    assert!(gate.finish(session(1)).is_err());
    assert!(gate.can_configure().is_err());
    let mut wrong = frame(1, 1);
    wrong.stamp.epoch.session = session(3);
    assert_eq!(gate.offer(wrong), OfferResult::DiscardedStale);
    let foreign = SessionId {
        instance: InstanceId(NonZeroU64::new(2).unwrap()),
        ..session(2)
    };
    assert!(gate.finish(foreign).is_err());
    gate.finish(session(2)).unwrap();
    gate.finished(session(2));
    gate.can_configure().unwrap();
}

#[tokio::test]
async fn future_epoch_waits_for_exact_admission_and_old_frames_stay_fenced() {
    let mut output = Running::new().await;
    output.hold().await;
    assert_eq!(
        output.colors.offer(frame(3, 2)).unwrap(),
        OfferResult::Queued
    );
    output.admit(1, EdgeColors::default()).await;
    assert!(output.calls.try_recv().is_err());
    output.admit(3, red()).await;
    assert_eq!(
        output.colors.offer(frame(3, 1)).unwrap(),
        OfferResult::DiscardedStale
    );
    assert_eq!(
        output.colors.offer(frame(3, 2)).unwrap(),
        OfferResult::DiscardedStale
    );
    let hold = output.hold_request(3);
    output.call(Kind::Colors(EdgeColors::default())).await.ok();
    within(hold).await.unwrap().unwrap();
    assert_eq!(
        output.colors.offer(frame(3, 9)).unwrap(),
        OfferResult::DiscardedStale
    );
    assert_eq!(
        output.colors.offer(frame(4, 1)).unwrap(),
        OfferResult::Queued
    );
    output.off().await;
    assert_eq!(
        output.colors.offer(frame(4, 2)).unwrap(),
        OfferResult::DiscardedStale
    );
    output.close().await;
}

#[tokio::test]
async fn finish_waits_for_a_delayed_color_write_and_discards_queued_colors() {
    let mut output = Running::new().await;
    output.hold().await;
    output.admit(1, EdgeColors::default()).await;
    output.colors.offer(frame(1, 1)).unwrap();
    let delayed = output.call(Kind::Colors(red())).await;
    let finish = output.finish_request(IdleOutputMode::Off);
    for sequence in 2..100 {
        assert_eq!(
            output.colors.offer(frame(1, sequence)).unwrap(),
            OfferResult::Queued
        );
    }
    assert_eq!(
        within(output.control.observe())
            .await
            .unwrap()
            .initial
            .state
            .admitted,
        Some(epoch(1))
    );
    assert!(!finish.is_finished());
    delayed.ok();
    output.call(Kind::Mode(Mode::Off)).await.ok();
    within(finish).await.unwrap().unwrap();
    assert!(output.calls.try_recv().is_err());
    output.close().await;
}

#[tokio::test]
async fn caller_timeout_does_not_cancel_a_mode_write_or_let_off_overtake_it() {
    let mut output = Running::new().await;
    let control = output.control.clone();
    let timed_out = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_millis(10),
            control.hold(epoch(0), EdgeColors::default()),
        )
        .await
    });
    let delayed = output.call(Kind::Mode(Mode::Realtime)).await;
    assert!(within(timed_out).await.unwrap().is_err());
    let finish = output.finish_request(IdleOutputMode::Off);
    let state = within(output.control.observe())
        .await
        .unwrap()
        .initial
        .state;
    assert_eq!(state.confirmation, None);
    assert!(!finish.is_finished());
    delayed.ok();
    output.call(Kind::Colors(EdgeColors::default())).await.ok();
    output.call(Kind::Mode(Mode::Off)).await.ok();
    within(finish).await.unwrap().unwrap();
    output.close().await;
}

#[tokio::test]
async fn ambiguous_http_failure_poison_cannot_be_cleared_by_configure_or_finish() {
    let mut output = Running::new().await;
    let request = output.hold_request(0);
    output.call(Kind::Mode(Mode::Realtime)).await.fail(true);
    assert_eq!(
        within(request).await.unwrap().unwrap_err().kind(),
        prysm_contract::FailureKind::RecoveryRequired
    );
    let state = output.control.observe().await.unwrap().initial.state;
    assert_eq!(
        state.failure.unwrap().kind,
        prysm_contract::FailureKind::RecoveryRequired
    );
    assert_eq!(state.confirmation, None);
    assert_eq!(
        output
            .control
            .configure(settings())
            .await
            .unwrap_err()
            .kind(),
        prysm_contract::FailureKind::RecoveryRequired
    );
    assert_eq!(
        output
            .control
            .finish(session(1), IdleOutputMode::Off)
            .await
            .unwrap_err()
            .kind(),
        prysm_contract::FailureKind::RecoveryRequired
    );
    assert!(output.calls.try_recv().is_err());
    output.shutdown.take().unwrap().send(()).unwrap();
    assert!(within(output.task).await.unwrap().is_err());
}

#[tokio::test]
async fn completed_mode_rejection_and_udp_failure_still_allow_cleanup() {
    for mode_failure in [true, false] {
        let mut output = Running::new().await;
        let request = output.hold_request(0);
        let realtime = output.call(Kind::Mode(Mode::Realtime)).await;
        if mode_failure {
            realtime.fail(false);
        } else {
            realtime.ok();
            output
                .call(Kind::Colors(EdgeColors::default()))
                .await
                .fail(false);
        }
        assert!(within(request).await.unwrap().is_err());
        assert!(
            output
                .control
                .observe()
                .await
                .unwrap()
                .initial
                .state
                .failure
                .is_some()
        );
        output.off().await;
        let state = output.control.observe().await.unwrap().initial.state;
        assert_eq!(state.idle_mode, Some(IdleOutputMode::Off));
        assert_eq!(state.confirmation, Some(OutputApplied::Sent));
        assert!(state.failure.is_none());
        output.close().await;
    }
}

#[tokio::test(start_paused = true)]
async fn refresh_is_owned_and_finishes_before_final_mode_without_resuming_afterward() {
    let mut output = Running::new().await;
    output.hold().await;
    tokio::time::advance(Duration::from_millis(500)).await;
    let refresh = output.call(Kind::Colors(EdgeColors::default())).await;
    let finish = output.finish_request(IdleOutputMode::Off);
    assert!(!finish.is_finished());
    refresh.ok();
    output.call(Kind::Mode(Mode::Off)).await.ok();
    within(finish).await.unwrap().unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    assert!(output.calls.try_recv().is_err());
    output.close().await;
}

#[tokio::test]
async fn shutdown_settles_the_current_write_and_then_turns_off() {
    let mut output = Running::new().await;
    let request = output.hold_request(0);
    let delayed = output.call(Kind::Mode(Mode::Realtime)).await;
    output.shutdown.take().unwrap().send(()).unwrap();
    let control = output.control.clone();
    let queued = tokio::spawn(async move { control.admit(epoch(1)).await });
    assert!(!output.task.is_finished());
    assert!(within(output.control.observe()).await.is_ok());
    delayed.ok();
    assert!(within(request).await.unwrap().is_err());
    output.call(Kind::Mode(Mode::Off)).await.ok();
    assert!(within(queued).await.unwrap().is_err());
    output.close().await;
}

#[tokio::test]
async fn terminal_fence_is_immediate_irreversible_and_keeps_failed_writes_visible() {
    for failed in [false, true] {
        let mut output = Running::new().await;
        output.hold().await;
        output.admit(1, EdgeColors::default()).await;
        output.colors.offer(frame(1, 1)).unwrap();
        let write = output.call(Kind::Colors(red())).await;
        let control = output.control.clone();
        let queued = tokio::spawn(async move { control.admit(epoch(2)).await });
        tokio::task::yield_now().await;
        output.terminal.request();
        output.terminal.request();
        assert!(output.colors.offer(frame(2, 2)).is_err());
        assert!(within(output.control.admit(epoch(3))).await.is_err());
        assert!(within(output.control.configure(settings())).await.is_err());
        assert!(within(output.control.hold(epoch(4), red())).await.is_err());
        assert!(
            within(output.control.finish(session(1), IdleOutputMode::Native))
                .await
                .is_err()
        );
        assert_eq!(
            output
                .control
                .observe()
                .await
                .unwrap()
                .initial
                .state
                .admitted,
            None
        );
        assert!(!output.task.is_finished());
        assert!(output.calls.try_recv().is_err());
        if failed {
            write.fail(false);
        } else {
            write.ok();
        }
        output.call(Kind::Mode(Mode::Off)).await.ok();
        assert!(within(queued).await.unwrap().is_err());
        let result = within(output.task).await.unwrap();
        assert_eq!(result.is_err(), failed);
        if let Err(error) = result {
            assert!(error.to_string().contains("test write failed"));
        }
        assert!(output.calls.try_recv().is_err());
    }
}

#[tokio::test]
async fn terminal_request_before_run_rejects_work_without_configuring_transport() {
    let output = WledOutput::new(INSTANCE);
    output.shutdown.request();
    assert!(output.control.configure(settings()).await.is_err());
    let (wire, mut calls) = mpsc::unbounded_channel();
    owner::run(output.owner, GatedWire(wire), std::future::pending())
        .await
        .unwrap();
    assert!(calls.try_recv().is_err());
}

#[tokio::test]
async fn shutdown_preserves_both_write_failures_and_the_recovery_category() {
    let mut output = Running::new().await;
    let request = output.hold_request(0);
    let write = output.call(Kind::Mode(Mode::Realtime)).await;
    output.terminal.request();
    write.fail(false);
    assert!(within(request).await.unwrap().is_err());
    output.call(Kind::Mode(Mode::Off)).await.fail(true);
    let error = within(output.task).await.unwrap().unwrap_err();
    assert_eq!(error.kind(), prysm_contract::FailureKind::RecoveryRequired);
    assert!(error.to_string().contains("cleanup also failed"));
}

#[tokio::test]
async fn snapshots_are_atomic_gaps_are_explicit_and_owner_closure_ends_observation() {
    let mut output = Running::new().await;
    output.hold().await;
    let mut observation = output.control.observe().await.unwrap();
    for _ in 0..20 {
        let request = output.hold_request(0);
        output.call(Kind::Colors(EdgeColors::default())).await.ok();
        within(request).await.unwrap().unwrap();
    }
    assert!(matches!(
        observation.updates.next().await.unwrap().unwrap(),
        Update::ResyncRequired
    ));
    assert!(observation.updates.next().await.is_none());
    let mut fresh = output.control.observe().await.unwrap();
    assert!(fresh.initial.state.held);
    output.off().await;
    let control = output.control.clone();
    output.close().await;
    within(async { while fresh.updates.next().await.is_some() {} }).await;
    assert!(matches!(
        control.observe().await,
        Err(ControlError::Unavailable(_))
    ));
}

#[tokio::test]
async fn rejected_finish_retains_device_and_session_until_cleanup_succeeds() {
    let mut output = Running::new().await;
    output.hold().await;
    let finish = output.finish_request(IdleOutputMode::Off);
    output.call(Kind::Mode(Mode::Off)).await.fail(false);
    assert!(within(finish).await.unwrap().is_err());
    let mut next_device = settings();
    next_device.address = "127.0.0.2".parse().unwrap();
    assert!(output.control.configure(next_device.clone()).await.is_err());
    let next_epoch = StreamEpoch {
        session: session(2),
        generation: 0,
    };
    assert!(matches!(
        output.control.hold(next_epoch, EdgeColors::default()).await,
        Err(ControlError::CleanupPending(_))
    ));
    assert!(output.calls.try_recv().is_err());
    output.off().await;
    output.control.configure(next_device).await.unwrap();
    output.call(Kind::Configure).await;
    output.shutdown.take().unwrap().send(()).unwrap();
    output.call(Kind::Mode(Mode::Off)).await.ok();
    output.close().await;
}
