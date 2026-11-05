//! Explicit test fixtures, never a production adapter fallback.
//! They record control calls and carry one queued frame; they do not emulate
//! hardware cleanup, epoch fences, subscriptions, retries, or device confirmation.
use prysm_contract::{
    ControlError, OwnerStopped,
    capture::{
        CaptureCapabilities, CaptureControl, CaptureStart, CaptureStarted, CaptureState,
        CaptureStopped, CapturedFrame, FrameSource,
    },
    identity::{SessionId, SettingsRevision, StreamEpoch},
    observation::Observation,
    output::{
        ColorFrame, ColorSink, IdleOutputMode, OfferResult, OutputApplied, OutputCapabilities,
        OutputControl, OutputState,
    },
    storage::SettingsRepository,
};
use prysm_core::EdgeColors;
use prysm_flow::FlowWiring;
use std::{
    future::Future,
    marker::PhantomData,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    CaptureStart(SessionId),
    CaptureStop(SessionId),
    OutputConfigure,
    OutputAdmit(StreamEpoch),
    OutputHold(StreamEpoch),
    OutputFinish(SessionId, IdleOutputMode),
}

#[derive(Default, Clone)]
pub struct Calls(Arc<Mutex<Vec<Call>>>);

impl Calls {
    pub fn recorded(&self) -> Vec<Call> {
        self.0.lock().expect("fixture lock").clone()
    }
    fn record(&self, call: Call) {
        self.0.lock().expect("fixture lock").push(call);
    }
}

pub struct FixtureOwner;

impl FixtureOwner {
    /// No resources exist in this fixture. A real owner must join its workers here.
    pub async fn run(
        self,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        shutdown.await;
        Ok(OwnerStopped)
    }
}

pub struct FakeCapture<S> {
    pub owner: FixtureOwner,
    pub control: FakeCaptureControl<S>,
    pub frames: FixtureFrames,
    pub feed: FrameFeed,
}

impl<S: Clone + Send + Sync + 'static> FakeCapture<S> {
    pub fn new(capabilities: CaptureCapabilities, calls: Calls) -> Self {
        let (sender, receiver) = mpsc::channel(1);
        Self {
            owner: FixtureOwner,
            control: FakeCaptureControl {
                capabilities,
                calls,
                settings: PhantomData,
            },
            frames: FixtureFrames(receiver),
            feed: FrameFeed(sender),
        }
    }
}

#[derive(Clone)]
pub struct FakeCaptureControl<S> {
    capabilities: CaptureCapabilities,
    calls: Calls,
    settings: PhantomData<fn() -> S>,
}

impl<S: Clone + Send + Sync + 'static> CaptureControl for FakeCaptureControl<S> {
    type Settings = S;
    fn capabilities(&self) -> CaptureCapabilities {
        self.capabilities.clone()
    }
    fn validate(&self, _settings: &S) -> Result<(), ControlError> {
        Ok(())
    }
    async fn start(&self, request: CaptureStart<S>) -> Result<CaptureStarted, ControlError> {
        self.calls.record(Call::CaptureStart(request.session));
        Ok(CaptureStarted {
            session: request.session,
        })
    }
    async fn stop(&self, session: SessionId) -> Result<CaptureStopped, ControlError> {
        self.calls.record(Call::CaptureStop(session));
        Ok(CaptureStopped { session })
    }
    async fn observe(&self) -> Result<Observation<CaptureState>, ControlError> {
        Err(ControlError::Unavailable(
            "fixture: live capture observation is not simulated",
        ))
    }
}

pub struct FixtureFrames(mpsc::Receiver<CapturedFrame>);

impl FrameSource for FixtureFrames {
    async fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ControlError> {
        Ok(self.0.recv().await)
    }
}

pub struct FrameFeed(mpsc::Sender<CapturedFrame>);

impl FrameFeed {
    pub async fn send(&self, frame: CapturedFrame) -> Result<(), ControlError> {
        self.0
            .send(frame)
            .await
            .map_err(|_| ControlError::Unavailable("fixture frame source closed"))
    }
}

pub struct FakeOutput<S> {
    pub owner: FixtureOwner,
    pub control: FakeOutputControl<S>,
    pub colors: RecordingColors,
    pub received: ReceivedColors,
}

impl<S: Clone + Send + Sync + 'static> FakeOutput<S> {
    pub fn new(calls: Calls) -> Self {
        let received = ReceivedColors::default();
        Self {
            owner: FixtureOwner,
            control: FakeOutputControl {
                calls,
                settings: PhantomData,
            },
            colors: RecordingColors(received.clone()),
            received,
        }
    }
}

#[derive(Clone)]
pub struct FakeOutputControl<S> {
    calls: Calls,
    settings: PhantomData<fn() -> S>,
}

impl<S: Clone + Send + Sync + 'static> OutputControl for FakeOutputControl<S> {
    type Settings = S;
    fn capabilities(&self) -> OutputCapabilities {
        OutputCapabilities { native_mode: true }
    }
    fn validate(&self, _settings: &S) -> Result<(), ControlError> {
        Ok(())
    }
    async fn configure(&self, _settings: S) -> Result<OutputApplied, ControlError> {
        self.calls.record(Call::OutputConfigure);
        Ok(OutputApplied::Sent)
    }
    async fn admit(&self, epoch: StreamEpoch) -> Result<OutputApplied, ControlError> {
        self.calls.record(Call::OutputAdmit(epoch));
        Ok(OutputApplied::Sent)
    }
    async fn hold(
        &self,
        epoch: StreamEpoch,
        _colors: EdgeColors,
    ) -> Result<OutputApplied, ControlError> {
        self.calls.record(Call::OutputHold(epoch));
        Ok(OutputApplied::Sent)
    }
    async fn finish(
        &self,
        session: SessionId,
        mode: IdleOutputMode,
    ) -> Result<OutputApplied, ControlError> {
        self.calls.record(Call::OutputFinish(session, mode));
        Ok(OutputApplied::Sent)
    }
    async fn observe(&self) -> Result<Observation<OutputState>, ControlError> {
        Err(ControlError::Unavailable(
            "fixture: live output observation is not simulated",
        ))
    }
}

#[derive(Default, Clone)]
pub struct ReceivedColors(Arc<Mutex<Option<ColorFrame>>>);

impl ReceivedColors {
    pub fn latest(&self) -> Option<ColorFrame> {
        self.0.lock().expect("fixture lock").clone()
    }
}

pub struct RecordingColors(ReceivedColors);

impl ColorSink for RecordingColors {
    fn offer(&self, frame: ColorFrame) -> Result<OfferResult, ControlError> {
        *self.0.0.lock().expect("fixture lock") = Some(frame);
        Ok(OfferResult::Queued)
    }
}

#[derive(Default)]
pub struct MemorySettings<S>(Mutex<Option<S>>);

impl<S> MemorySettings<S> {
    pub fn empty() -> Self {
        Self(Mutex::new(None))
    }
}

impl<S: Clone + Send + 'static> SettingsRepository<S> for MemorySettings<S> {
    async fn load(&self) -> Result<Option<S>, ControlError> {
        Ok(self.0.lock().expect("fixture lock").clone())
    }
    async fn save(&self, settings: S) -> Result<(), ControlError> {
        *self.0.lock().expect("fixture lock") = Some(settings);
        Ok(())
    }
}

/// One-frame harness, not the production flow driver. It proves endpoint compatibility
/// using the existing processor, without implementing scheduling or session policy.
pub async fn process_one<F: FrameSource, O: ColorSink>(
    wiring: &mut FlowWiring<F, O>,
    revision: SettingsRevision,
) -> Result<bool, ControlError> {
    let Some(captured) = wiring.frames.next_frame().await? else {
        return Ok(false);
    };
    let colors = wiring.processor.process_frame(captured.frame);
    wiring.colors.offer(ColorFrame {
        stamp: captured.stamp,
        revision,
        colors,
    })?;
    Ok(true)
}
