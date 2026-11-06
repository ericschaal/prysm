//! Serial I/O execution. A dropped caller loses its reply, never the accepted work.
use crate::{admission::Admission, disk::Store, format};
use prysm_contract::{ControlError, Failure, FailureKind, OwnerStopped, application::Settings};
use serde::{Serialize, de::DeserializeOwned};
use std::sync::{Weak, mpsc};
use tokio::sync::oneshot;

pub(super) enum Request<S> {
    Load(oneshot::Sender<Result<Option<S>, Failure>>),
    Save(S, oneshot::Sender<Result<(), Failure>>),
}

pub(super) fn run<C: Serialize + DeserializeOwned, O: Serialize + DeserializeOwned>(
    mut store: Store,
    receiver: mpsc::Receiver<Request<Settings<C, O>>>,
    admission: Weak<Admission<Settings<C, O>>>,
) -> Result<OwnerStopped, ControlError> {
    let mut last_failure = None;
    let mut uncertain: Option<Failure> = None;
    while let Ok(request) = receiver.recv() {
        match request {
            Request::Load(reply) => {
                let result = match &uncertain {
                    Some(failure) => Err(failure.clone()),
                    None => load_settings(&store).map_err(Failure::from),
                };
                retain_failure(&result, &mut last_failure);
                let _ = reply.send(result);
            }
            Request::Save(settings, reply) => {
                let result = match &uncertain {
                    Some(failure) => Err(failure.clone()),
                    None => save_settings(&mut store, settings).map_err(Failure::from),
                };
                if let Err(failure) = &result
                    && failure.kind == FailureKind::RecoveryRequired
                {
                    uncertain = Some(failure.clone());
                    if let Some(admission) = admission.upgrade() {
                        admission.close_with_failure(failure.clone());
                    }
                }
                retain_failure(&result, &mut last_failure);
                let _ = reply.send(result);
            }
        }
    }
    match uncertain.or(last_failure) {
        Some(failure) => Err(failure.into()),
        None => Ok(OwnerStopped),
    }
}

fn load_settings<C: DeserializeOwned, O: DeserializeOwned>(
    store: &Store,
) -> Result<Option<Settings<C, O>>, ControlError> {
    let Some(bytes) = store.load()? else {
        return Ok(None);
    };
    let settings = format::decode(&bytes)?;
    Ok(Some(settings))
}

fn save_settings<C: Serialize, O: Serialize>(
    store: &mut Store,
    settings: Settings<C, O>,
) -> Result<(), ControlError> {
    let bytes = format::encode(settings)?;
    store.save(&bytes)
}

fn retain_failure<T>(result: &Result<T, Failure>, retained: &mut Option<Failure>) {
    // Delivery to a reply channel does not prove the caller observed the failure.
    if let Err(failure) = result {
        *retained = Some(failure.clone());
    }
}
