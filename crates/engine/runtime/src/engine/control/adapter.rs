use crate::session::Lease;
use prysm_ports::{
    AdapterLifecycle, Availability, Claim, ConfiguredAdapter, Error, Hangup,
    OutputObservationState, Readiness, Result,
};
use std::sync::Arc;
use tokio::task::JoinHandle;

pub(super) struct Effect<T> {
    stop: Hangup,
    task: JoinHandle<T>,
}

impl<T: Send + 'static> Effect<T> {
    pub(super) fn start(f: impl FnOnce(&Hangup) -> T + Send + 'static) -> Self {
        let stop = Hangup::default();
        let run = stop.clone();
        Self {
            stop,
            task: tokio::task::spawn_blocking(move || f(&run)),
        }
    }

    pub(super) async fn next(effect: &mut Option<Self>) -> Result<T> {
        let Some(active) = effect.as_mut() else {
            return std::future::pending().await;
        };
        let result = (&mut active.task)
            .await
            .map_err(|_| Error::Panicked("lighting effect"));
        *effect = None;
        result
    }
}

impl<T> Drop for Effect<T> {
    fn drop(&mut self) {
        self.stop.cut();
    }
}

pub(super) type Observed = Option<(Readiness<Error>, OutputObservationState)>;

pub(super) struct Adapter<P: ConfiguredAdapter + ?Sized> {
    pub(super) port: Result<Arc<P>>,
    lifecycle: AdapterLifecycle<Error>,
    check: Option<Effect<Observed>>,
    watching: bool,
    open_failed: bool,
}

impl<P: ConfiguredAdapter + ?Sized + 'static> Adapter<P> {
    pub(super) fn new(port: Result<Arc<P>>) -> Self {
        Self {
            port,
            lifecycle: AdapterLifecycle::default(),
            check: None,
            watching: false,
            open_failed: false,
        }
    }

    pub(super) fn failed(&mut self, error: Error) {
        let ready = match &self.port {
            Ok(port) => port.failure(error),
            Err(_) => Readiness::Blocked(error),
        };
        self.observe(ready);
    }

    pub(super) fn blocker(&self) -> Option<&Error> {
        match self.available() {
            Availability::Blocked => self.reason(),
            _ => None,
        }
    }

    pub(super) fn available(&self) -> Availability {
        self.lifecycle.availability()
    }

    pub(super) fn reason(&self) -> Option<&Error> {
        match self.lifecycle.readiness() {
            Some(Readiness::Waiting(error) | Readiness::Blocked(error)) => Some(error),
            Some(Readiness::Ready) | None => None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.cancel();
        self.lifecycle.stop();
        self.open_failed = false;
    }

    pub(super) fn cancel(&mut self) {
        self.check = None;
        self.watching = false;
    }

    pub(super) fn cancel_watch(&mut self) {
        if self.watching {
            self.cancel();
        }
    }

    pub(super) fn check(&mut self) {
        if self.check.is_some() || self.available() == Availability::Active {
            return;
        }
        match self.port.clone() {
            Ok(port) => self.check = Some(Effect::start(move |stop| Some(port.observe(stop)))),
            Err(error) => self.observe(Readiness::Blocked(error)),
        }
    }

    pub(super) fn monitor(&mut self, needed: bool) {
        if !needed {
            self.cancel_watch();
        } else if self.available() == Availability::Down {
            self.check();
        } else if self.available() == Availability::Waiting
            && self.check.is_none()
            && let Ok(port) = self.port.clone()
        {
            self.watching = true;
            self.check = Some(Effect::start(move |stop| port.watch(stop)));
        }
    }

    pub(super) async fn next(&mut self) -> Observed {
        let observed = Effect::next(&mut self.check).await;
        self.watching = false;
        match observed {
            Ok(Some(observed)) => Some(observed),
            Ok(None) => self.reason().cloned().map(|reason| {
                (
                    Readiness::Blocked(reason),
                    OutputObservationState::Unsupported,
                )
            }),
            Err(error) => Some((
                Readiness::Blocked(error),
                OutputObservationState::Unsupported,
            )),
        }
    }

    pub(super) fn observe(&mut self, readiness: Readiness<Error>) {
        if !matches!(readiness, Readiness::Ready) {
            self.open_failed = false;
        }
        if self.lifecycle.readiness().is_none() {
            self.lifecycle.start(readiness);
        } else {
            self.lifecycle.observe(readiness);
        }
    }

    /// An open which failed while still reporting Ready needs a fresh reason to try.
    pub(super) fn failed_open(&mut self) {
        self.open_failed = matches!(self.lifecycle.readiness(), Some(Readiness::Ready));
    }

    pub(super) fn allow_open(&mut self) {
        self.open_failed = false;
    }

    pub(super) fn can_retry_open(&self) -> bool {
        !self.open_failed
    }

    pub(super) fn claim(&mut self) -> Option<Claim> {
        let claim = self.lifecycle.claim()?;
        self.cancel();
        Some(claim)
    }

    pub(super) fn teardown(&mut self, lease: &mut Lease) {
        if let Lease::Claimed(claim) = lease
            && let Some(release) = self.lifecycle.begin_release(claim)
        {
            *lease = Lease::Releasing(release);
        }
    }

    /// An adapter the session never opened requires no release.
    pub(super) fn release(&mut self, lease: Lease) -> bool {
        match lease {
            Lease::Unclaimed => self.available() != Availability::Active,
            Lease::Releasing(release) => self.lifecycle.released(&release).is_some(),
            Lease::Claimed(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_an_active_adapter_requires_its_sessions_proof() {
        let mut capture =
            Adapter::<dyn prysm_ports::ConfiguredSource>::new(Err(Error::other("unused")));
        let mut output =
            Adapter::<dyn prysm_ports::ConfiguredOutput>::new(Err(Error::other("unused")));
        capture.observe(Readiness::Ready);
        output.observe(Readiness::Ready);
        let mut capture_lease = Lease::Claimed(capture.claim().unwrap());
        let mut output_lease = Lease::Claimed(output.claim().unwrap());
        assert!(capture.claim().is_none());
        output.teardown(&mut capture_lease);
        capture.teardown(&mut capture_lease);
        output.teardown(&mut output_lease);
        assert!(!output.release(Lease::Unclaimed));
        assert!(!output.release(capture_lease));
        assert!(output.claim().is_none());
        assert!(output.release(output_lease));
        assert_eq!(output.available(), Availability::Ready);
        assert_eq!(capture.available(), Availability::Active);
    }

    #[test]
    fn a_failed_open_waits_for_its_own_readiness_change_or_explicit_recheck() {
        let slot =
            || Adapter::<dyn prysm_ports::ConfiguredSource>::new(Err(Error::other("unused port")));
        let mut failed = slot();
        let mut peer = slot();
        for slot in [&mut failed, &mut peer] {
            slot.observe(Readiness::Ready);
        }
        let mut lease = Lease::Claimed(failed.claim().unwrap());
        failed.failed_open();
        failed.teardown(&mut lease);
        assert!(failed.release(lease));
        peer.observe(Readiness::Waiting(Error::other("offline")));
        peer.observe(Readiness::Ready);
        failed.observe(Readiness::Ready);
        assert!(!failed.can_retry_open());

        failed.observe(Readiness::Waiting(Error::other("unplugged")));
        failed.observe(Readiness::Ready);
        assert!(failed.can_retry_open());
        failed.failed_open();
        failed.reset();
        assert_eq!(failed.available(), Availability::Down);
        assert!(failed.can_retry_open());
    }
}
