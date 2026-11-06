//! Saved settings and one retained replacement. Device sequencing belongs to Lifecycle.
use futures::future::BoxFuture;
use prysm_contract::{
    ControlError, Failure, FailureKind,
    identity::{OperationId, SettingsRevision},
    storage::SettingsRepository,
};

type Save<R> = BoxFuture<'static, (R, Result<(), ControlError>)>;
enum Phase<S, R> {
    Ready(R),
    Saving {
        candidate: S,
        revision: SettingsRevision,
        id: OperationId,
        interruption: Option<Failure>,
        future: Save<R>,
    },
    Applying {
        repository: R,
        id: OperationId,
        interruption: Option<Failure>,
    },
}

pub(super) struct SettingsWorkflow<S, R> {
    saved: S,
    revision: SettingsRevision,
    // Taken only during synchronous transitions; no await leaves this empty.
    phase: Option<Phase<S, R>>,
}
pub(super) enum Saved {
    Committed,
    Finished(OperationId, Failure),
}
impl<S: Clone + Send + 'static, R: SettingsRepository<S>> SettingsWorkflow<S, R> {
    pub fn new(saved: S, repository: R) -> Self {
        Self {
            saved,
            revision: SettingsRevision(0),
            phase: Some(Phase::Ready(repository)),
        }
    }
    pub fn saved(&self) -> &S {
        &self.saved
    }
    pub fn revision(&self) -> SettingsRevision {
        self.revision
    }
    pub fn busy(&self) -> bool {
        !matches!(self.phase, Some(Phase::Ready(_)))
    }
    pub fn next_revision(
        &self,
        expected: SettingsRevision,
    ) -> Result<SettingsRevision, ControlError> {
        if self.busy() {
            return Err(ControlError::Conflict(
                "settings replacement is in progress",
            ));
        }
        if expected != self.revision {
            return Err(ControlError::Conflict("settings revision changed"));
        }
        self.revision
            .0
            .checked_add(1)
            .map(SettingsRevision)
            .ok_or(ControlError::Conflict("settings revision exhausted"))
    }
    pub fn begin(&mut self, id: OperationId, candidate: S, revision: SettingsRevision) {
        let Some(Phase::Ready(repository)) = self.phase.take() else {
            unreachable!("replacement validated before acceptance")
        };
        let value = candidate.clone();
        // Own R across the await; repositories need neither Clone nor Sync.
        let future = Box::pin(async move {
            let result = repository.save(value).await;
            (repository, result)
        });
        self.phase = Some(Phase::Saving {
            candidate,
            revision,
            id,
            interruption: None,
            future,
        });
    }
    pub fn interrupt(&mut self, failure: Failure) {
        let interruption = match self.phase.as_mut() {
            Some(Phase::Saving { interruption, .. } | Phase::Applying { interruption, .. }) => {
                interruption
            }
            _ => return,
        };
        if interruption
            .as_ref()
            .is_none_or(|prior| prior.kind == FailureKind::Interrupted)
        {
            *interruption = Some(failure);
        }
    }
    /// Cancel-safe waiter: losing select never drops the save future.
    pub async fn progress(&mut self) -> Saved {
        let (repository, result) = match self.phase.as_mut() {
            Some(Phase::Saving { future, .. }) => future.as_mut().await,
            _ => return std::future::pending().await,
        };
        let Some(Phase::Saving {
            candidate,
            revision,
            id,
            interruption,
            ..
        }) = self.phase.take()
        else {
            unreachable!()
        };
        match result {
            Err(error) => {
                self.phase = Some(Phase::Ready(repository));
                Saved::Finished(id, error.into())
            }
            Ok(()) => {
                self.saved = candidate;
                self.revision = revision;
                if let Some(failure) = interruption {
                    self.phase = Some(Phase::Ready(repository));
                    Saved::Finished(id, failure)
                } else {
                    self.phase = Some(Phase::Applying {
                        repository,
                        id,
                        interruption: None,
                    });
                    Saved::Committed
                }
            }
        }
    }
    pub fn applied(&mut self, result: Result<(), Failure>) -> (OperationId, Option<Failure>) {
        let Some(Phase::Applying {
            repository,
            id,
            interruption,
        }) = self.phase.take()
        else {
            unreachable!("application follows acknowledged save")
        };
        self.phase = Some(Phase::Ready(repository));
        (id, result.err().or(interruption))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Repository;
    impl SettingsRepository<()> for Repository {
        async fn load(&self) -> Result<Option<()>, ControlError> {
            Ok(None)
        }
        async fn save(&self, _: ()) -> Result<(), ControlError> {
            panic!("overflow must not save")
        }
    }
    #[test]
    fn revision_exhaustion_rejects_before_saving() {
        let mut settings = SettingsWorkflow::new((), Repository);
        settings.revision = SettingsRevision(u64::MAX);
        assert!(settings.next_revision(settings.revision()).is_err());
        assert!(!settings.busy());
    }
}
