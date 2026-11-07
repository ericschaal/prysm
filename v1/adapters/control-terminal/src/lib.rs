//! Driving adapter: terminal text becomes typed application commands.
//! The host allocates operation IDs and submits; the application saves and applies.
use futures::{FutureExt, StreamExt, future::BoxFuture, stream::BoxStream};
use prysm_contract::{
    ControlError,
    application::{ApplicationCommand, ApplicationControl, LightingTarget, Settings},
};
use prysm_protocol::settings::SettingsDocument;
use serde::de::DeserializeOwned;
use std::{future::pending, path::Path, time::Duration};
use tokio::{io::AsyncReadExt, sync::mpsc};

const MAX_BYTES: u64 = 1024 * 1024;
type Prepared<S> = Result<ApplicationCommand<S>, ControlError>;

/// One bounded load may be preparing while on/off remain responsive. Dropping this
/// stream stops preparation; an already submitted operation belongs to the application.
pub fn commands<A, C, O>(
    control: A,
    input: mpsc::Receiver<String>,
) -> BoxStream<'static, Prepared<Settings<C, O>>>
where
    A: ApplicationControl<Settings = Settings<C, O>>,
    C: DeserializeOwned + Send + 'static,
    O: DeserializeOwned + Send + 'static,
{
    let input = Input {
        control,
        input,
        loading: None,
    };
    futures::stream::unfold(input, |mut input| async move {
        let command = input.next().await?;
        Some((command, input))
    })
    .boxed()
}

struct Input<A: ApplicationControl> {
    control: A,
    input: mpsc::Receiver<String>,
    loading: Option<BoxFuture<'static, Prepared<A::Settings>>>,
}

impl<A, C, O> Input<A>
where
    A: ApplicationControl<Settings = Settings<C, O>>,
    C: DeserializeOwned + Send + 'static,
    O: DeserializeOwned + Send + 'static,
{
    async fn next(&mut self) -> Option<Prepared<A::Settings>> {
        loop {
            tokio::select! {
                line = self.input.recv() => {
                    if let Some(command) = self.accept_line(line?) {
                        return Some(command);
                    }
                }
                result = prepared(&mut self.loading) => {
                    self.loading = None;
                    return Some(result);
                }
            }
        }
    }

    /// Targets/errors are ready now; a load retains its preparation for the next poll.
    fn accept_line(&mut self, line: String) -> Option<Prepared<A::Settings>> {
        match line.trim() {
            "on" => Some(Ok(ApplicationCommand::SetTarget(LightingTarget::Processed))),
            "off" => Some(Ok(ApplicationCommand::SetTarget(LightingTarget::Off))),
            line if line.starts_with("load ") => {
                if self.loading.is_some() {
                    return Some(Err(ControlError::Conflict(
                        "a settings file is already loading",
                    )));
                }
                let path = line[5..].trim().to_owned();
                let control = self.control.clone();
                self.loading =
                    Some(async move { prepare(&control, Path::new(&path)).await }.boxed());
                None
            }
            _ => Some(Err(invalid("use on, off, or load <path>"))),
        }
    }
}

async fn prepared<S>(loading: &mut Option<BoxFuture<'static, Prepared<S>>>) -> Prepared<S> {
    match loading {
        Some(loading) => loading.await,
        None => pending().await,
    }
}

async fn prepare<A, C, O>(control: &A, path: &Path) -> Prepared<Settings<C, O>>
where
    A: ApplicationControl<Settings = Settings<C, O>>,
    C: DeserializeOwned,
    O: DeserializeOwned,
{
    // Capture the revision before I/O so a concurrent save cannot be overwritten.
    let expected = control.observe().await?.initial.state.saved_revision;
    let bytes = tokio::time::timeout(Duration::from_secs(5), read_document(path))
        .await
        .map_err(|_| ControlError::Unavailable("settings file read timed out"))??;
    let document: SettingsDocument<C, O> =
        serde_json::from_slice(&bytes).map_err(|error| invalid(&error.to_string()))?;
    Ok(ApplicationCommand::ReplaceSettings {
        expected,
        settings: document.try_into()?,
    })
}

async fn read_document(path: &Path) -> Result<Vec<u8>, ControlError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    // Open first without blocking on FIFOs, then validate that exact handle.
    #[cfg(unix)]
    options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    let file = options.open(path).await.map_err(io_error)?;
    let metadata = file.metadata().await.map_err(io_error)?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(invalid("expected a regular settings file of at most 1 MiB"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("settings file exceeds 1 MiB"));
    }
    Ok(bytes)
}

fn invalid(reason: &str) -> ControlError {
    ControlError::Invalid {
        field: "terminal command",
        reason: reason.into(),
    }
}
fn io_error(error: std::io::Error) -> ControlError {
    ControlError::Adapter(Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn candidate_reads_reject_directories_missing_and_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_document(dir.path()).await.is_err());
        let path = dir.path().join("candidate.json");
        assert!(read_document(&path).await.is_err());
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(read_document(&path).await.is_err());
        std::fs::write(&path, b"{}").unwrap();
        assert_eq!(read_document(&path).await.unwrap(), b"{}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fifo_is_rejected_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let result = tokio::time::timeout(Duration::from_secs(1), read_document(&path))
            .await
            .unwrap();
        assert!(matches!(result, Err(ControlError::Invalid { .. })));
    }
}
