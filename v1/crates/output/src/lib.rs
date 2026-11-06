//! Local session/epoch fences and one newest candidate; no I/O or async work.
use prysm_contract::{
    ControlError,
    identity::{FrameStamp, InstanceId, SessionId, StreamEpoch},
    output::{ColorFrame, OfferResult},
};

struct Active {
    id: SessionId,
    control_epoch: u64,
    hold_floor: Option<u64>,
    admitted: Option<StreamEpoch>,
    newest: Option<FrameStamp>,
    candidate: Option<ColorFrame>,
}
enum Session {
    Idle,
    Active(Active),
    Finishing(SessionId),
}

/// Output session/epoch fences and one latest candidate. The adapter serializes
/// access and owns writes, acknowledgments, observations and terminal shutdown.
pub struct Admission {
    instance: InstanceId,
    sealed: u64,
    session: Session,
    suspended: bool,
}
impl Admission {
    pub fn new(instance: InstanceId) -> Self {
        Self {
            instance,
            sealed: 0,
            session: Session::Idle,
            suspended: false,
        }
    }
    pub fn can_configure(&self) -> Result<(), ControlError> {
        if !matches!(self.session, Session::Idle) {
            Err(ControlError::Conflict(
                "finish the active output session before configuring",
            ))
        } else {
            Ok(())
        }
    }
    fn check_id(&self, session: SessionId) -> Result<(), ControlError> {
        if session.instance != self.instance {
            return Err(ControlError::Conflict(
                "output session belongs to another instance",
            ));
        }
        match &self.session {
            Session::Active(active) if active.id != session => {
                return Err(ControlError::Conflict(
                    "another output session is unfinished",
                ));
            }
            Session::Finishing(pending) if *pending != session => {
                return Err(ControlError::CleanupPending(*pending));
            }
            _ => {}
        }
        Ok(())
    }
    fn epoch(&mut self, epoch: StreamEpoch) -> Result<&mut Active, ControlError> {
        self.check_id(epoch.session)?;
        if epoch.session.sequence.get() <= self.sealed {
            return Err(ControlError::Conflict("output session is sealed"));
        }
        if matches!(self.session, Session::Idle) {
            self.session = Session::Active(Active {
                id: epoch.session,
                control_epoch: 0,
                hold_floor: None,
                admitted: None,
                newest: None,
                candidate: None,
            });
        }
        let Session::Active(active) = &mut self.session else {
            return Err(ControlError::CleanupPending(epoch.session));
        };
        if epoch.generation < active.control_epoch {
            return Err(ControlError::Conflict("output epoch regressed"));
        }
        Ok(active)
    }
    pub fn hold(&mut self, epoch: StreamEpoch) -> Result<(), ControlError> {
        let active = self.epoch(epoch)?;
        active.control_epoch = epoch.generation;
        active.hold_floor = Some(epoch.generation);
        active.admitted = None;
        if active
            .candidate
            .as_ref()
            .is_some_and(|frame| frame.stamp.epoch.generation <= epoch.generation)
        {
            active.candidate = None;
        }
        self.suspended = false;
        Ok(())
    }
    pub fn admit(&mut self, epoch: StreamEpoch) -> Result<(), ControlError> {
        if epoch.generation == 0 {
            return Err(ControlError::Invalid {
                field: "epoch",
                reason: "epoch zero is initial waiting".into(),
            });
        }
        let active = self.epoch(epoch)?;
        if active
            .hold_floor
            .is_some_and(|floor| epoch.generation <= floor)
        {
            return Err(ControlError::Conflict("held epochs cannot be readmitted"));
        }
        active.control_epoch = epoch.generation;
        active.admitted = Some(epoch);
        if active
            .candidate
            .as_ref()
            .is_some_and(|frame| frame.stamp.epoch.generation < epoch.generation)
        {
            active.candidate = None;
        }
        self.suspended = false;
        Ok(())
    }
    pub fn finish(&mut self, session: SessionId) -> Result<(), ControlError> {
        self.check_id(session)?;
        if session.sequence.get() < self.sealed {
            return Err(ControlError::Conflict("output session is stale"));
        }
        self.sealed = session.sequence.get();
        self.session = Session::Finishing(session);
        Ok(())
    }
    pub fn finished(&mut self, session: SessionId) {
        assert!(
            matches!(self.session, Session::Finishing(pending) if pending == session),
            "finish acknowledgment belongs to retained session"
        );
        self.session = Session::Idle;
    }
    pub fn suspend(&mut self) {
        self.suspended = true;
        if let Session::Active(active) = &mut self.session {
            active.admitted = None;
            active.candidate = None;
        }
    }
    pub fn offer(&mut self, frame: ColorFrame) -> OfferResult {
        let Session::Active(active) = &mut self.session else {
            return OfferResult::DiscardedStale;
        };
        let stamp = frame.stamp;
        if self.suspended
            || stamp.epoch.session != active.id
            || stamp.epoch.generation == 0
            || stamp.epoch.generation < active.control_epoch
            || active
                .hold_floor
                .is_some_and(|floor| stamp.epoch.generation <= floor)
            || active.newest.is_some_and(|newest| {
                (stamp.epoch.generation, stamp.sequence)
                    <= (newest.epoch.generation, newest.sequence)
            })
        {
            return OfferResult::DiscardedStale;
        }
        active.newest = Some(stamp);
        active.candidate = Some(frame);
        OfferResult::Queued
    }
    pub fn take(&mut self) -> Option<ColorFrame> {
        let Session::Active(active) = &mut self.session else {
            return None;
        };
        if !self.suspended
            && active
                .candidate
                .as_ref()
                .is_some_and(|frame| Some(frame.stamp.epoch) == active.admitted)
        {
            active.candidate.take()
        } else {
            None
        }
    }
}
