//! Typed JSON persistence with one owned I/O thread. See README.md for guarantees.
mod admission;
mod disk;
mod format;
mod worker;

use admission::Admission;
use prysm_contract::{
    ControlError, Failure, OwnerStopped, application::Settings, storage::SettingsRepository,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{future::Future, path::PathBuf, sync::Arc, thread::JoinHandle};
use tokio::sync::oneshot;
use worker::Request;

pub struct FileSettings<C, O> {
    pub repository: FileSettingsHandle<Settings<C, O>>,
    pub owner: FileSettingsOwner,
    pub shutdown: FileSettingsShutdown,
}

impl<C, O> FileSettings<C, O>
where
    C: Serialize + DeserializeOwned + Send + 'static,
    O: Serialize + DeserializeOwned + Send + 'static,
{
    /// Starts an owned worker; the host must retain and run `owner` through shutdown.
    pub fn new(path: PathBuf) -> Result<Self, ControlError> {
        Self::open(disk::Store::new(path)?)
    }

    fn open(store: disk::Store) -> Result<Self, ControlError> {
        let (admission, receiver) = Admission::channel();
        let shutdown_admission = admission.clone();
        let shutdown = FileSettingsShutdown {
            close: Arc::new(move || shutdown_admission.close()),
        };
        let (done, finished) = oneshot::channel();
        let shared = Arc::downgrade(&admission);
        let join = std::thread::Builder::new()
            .name("prysm-settings".into())
            .spawn(move || {
                let result = worker::run(store, receiver, shared);
                let _ = done.send(());
                result
            })
            .map_err(|error| ControlError::Adapter(Box::new(error)))?;
        Ok(Self {
            repository: FileSettingsHandle { admission },
            owner: FileSettingsOwner {
                join: Some(join),
                finished,
                shutdown: shutdown.clone(),
            },
            shutdown,
        })
    }
}

pub struct FileSettingsHandle<S> {
    admission: Arc<Admission<S>>,
}
impl<S> Clone for FileSettingsHandle<S> {
    fn clone(&self) -> Self {
        Self {
            admission: self.admission.clone(),
        }
    }
}
impl<S: Send + 'static> FileSettingsHandle<S> {
    async fn request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, Failure>>) -> Request<S>,
    ) -> Result<T, ControlError> {
        let (reply, result) = oneshot::channel();
        self.admission.send(build(reply))?;
        result
            .await
            .map_err(|_| {
                ControlError::RecoveryRequired(Box::new(std::io::Error::other(
                    "settings worker stopped before replying; commit outcome unknown",
                )))
            })?
            .map_err(Into::into)
    }
}
impl<C: Send + 'static, O: Send + 'static> SettingsRepository<Settings<C, O>>
    for FileSettingsHandle<Settings<C, O>>
{
    async fn load(&self) -> Result<Option<Settings<C, O>>, ControlError> {
        self.request(Request::Load).await
    }
    async fn save(&self, settings: Settings<C, O>) -> Result<(), ControlError> {
        self.request(|reply| Request::Save(settings, reply)).await
    }
}

/// Synchronously fences admission. Already accepted requests remain owned and drain.
#[derive(Clone)]
pub struct FileSettingsShutdown {
    close: Arc<dyn Fn() + Send + Sync>,
}
impl FileSettingsShutdown {
    pub fn request(&self) {
        (self.close)();
    }
}

pub struct FileSettingsOwner {
    join: Option<JoinHandle<Result<OwnerStopped, ControlError>>>,
    finished: oneshot::Receiver<()>,
    shutdown: FileSettingsShutdown,
}
impl FileSettingsOwner {
    /// Await worker completion before joining. Cancellation falls back to a blocking join.
    pub async fn run(
        mut self,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        tokio::pin!(shutdown);
        tokio::select! {
            _ = &mut shutdown => {
                self.shutdown.request();
                let _ = (&mut self.finished).await;
            }
            _ = &mut self.finished => {}
        }
        self.shutdown.request();
        self.join
            .take()
            .expect("settings worker joined once")
            .join()
            .map_err(|_| {
                ControlError::RecoveryRequired(Box::new(std::io::Error::other(
                    "settings worker panicked; commit outcome unknown",
                )))
            })?
    }
}
impl Drop for FileSettingsOwner {
    fn drop(&mut self) {
        self.shutdown.request();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests;
