//! One shared latest snapshot. Only leases create demand; the worker owns closure.
use crate::CLOSED;
use futures::StreamExt;
use prysm_contract::{
    ControlError,
    identity::{FrameStamp, SettingsRevision},
    preview::{PreviewFrame, PreviewLease, PreviewSource},
};
use prysm_core::Frame;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

struct State {
    sender: Option<watch::Sender<()>>,
    latest: Option<PreviewFrame>,
    leases: usize,
}

/// Dropping the sole publisher closes subscriptions, including before the owner runs.
pub(super) struct Publisher(Arc<Mutex<State>>);

#[derive(Clone)]
pub struct PreviewHandle(Arc<Mutex<State>>);

impl Publisher {
    pub(super) fn new() -> (Self, PreviewHandle) {
        let (sender, _) = watch::channel(());
        let shared = Arc::new(Mutex::new(State {
            sender: Some(sender),
            latest: None,
            leases: 0,
        }));
        (Self(shared.clone()), PreviewHandle(shared))
    }

    pub(super) fn retain(&self, frame: &Frame) -> Option<Frame> {
        let state = self.0.lock().expect("preview state lock");
        (state.leases > 0).then(|| frame.clone())
    }

    pub(super) fn publish(&self, frame: PreviewFrame) {
        let sender = {
            let mut state = self.0.lock().expect("preview state lock");
            // Demand may disappear while processing; check again before retaining pixels.
            if state.leases == 0 {
                return;
            }
            state.latest = Some(frame);
            state.sender.as_ref().expect("live publisher").clone()
        };
        // A receiver's waker may drop its lease immediately and reenter this state.
        sender.send_replace(());
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        let sender = {
            let mut state = self.0.lock().expect("preview state lock");
            state.latest = None;
            state.sender.take()
        };
        // Closing the channel wakes receivers too; do it after unlocking.
        drop(sender);
    }
}

struct Subscription {
    shared: Arc<Mutex<State>>,
    receiver: watch::Receiver<()>,
    delivered: Option<(FrameStamp, SettingsRevision)>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut state = self.shared.lock().expect("preview state lock");
        state.leases -= 1;
        if state.leases == 0 {
            state.latest = None;
        }
    }
}

impl PreviewSource for PreviewHandle {
    async fn subscribe(&self) -> Result<PreviewLease, ControlError> {
        let mut state = self.0.lock().expect("preview state lock");
        let mut receiver = state.sender.as_ref().ok_or(CLOSED)?.subscribe();
        // A second viewer can immediately see the retained latest snapshot.
        receiver.mark_changed();
        state.leases += 1;
        let subscription = Subscription {
            shared: self.0.clone(),
            receiver,
            delivered: None,
        };
        Ok(PreviewLease::new(
            futures::stream::unfold(subscription, |mut subscription| async {
                loop {
                    subscription.receiver.changed().await.ok()?;
                    let frame = subscription
                        .shared
                        .lock()
                        .expect("preview state lock")
                        .latest
                        .clone();
                    if let Some(frame) = frame
                        && subscription.delivered != Some((frame.stamp, frame.revision))
                    {
                        // Publication and notification are separate: a newly subscribed
                        // viewer can already have read the frame before its wake arrives.
                        subscription.delivered = Some((frame.stamp, frame.revision));
                        return Some((Ok(frame), subscription));
                    }
                }
            })
            .boxed(),
        ))
    }
}
