//! Native ownership translated once at the adapter boundary.
use crate::DeviceCaptureSettings;
use futures::{FutureExt, future::BoxFuture};
use prysm_contract::{ControlError, Failure};
use prysm_core::Frame;
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

pub(super) struct Completion {
    pub failure: Option<Failure>,
    pub cleanup_complete: bool,
}

/// Private test seam: readiness and completion cannot be lost behind frame delivery.
pub(super) struct Run {
    pub ready: oneshot::Receiver<()>,
    pub frames: watch::Receiver<Option<Frame>>,
    pub stop: CancellationToken,
    pub finished: BoxFuture<'static, Completion>,
}
impl Drop for Run {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

pub(super) fn open(settings: &DeviceCaptureSettings) -> Result<Run, ControlError> {
    settings.validate()?;
    let DeviceCaptureSettings::Device {
        path,
        resolution,
        rate,
    } = settings
    else {
        unreachable!("validated device settings")
    };
    let [width, height] = resolution.dimensions();
    let prysm_capture::DeviceCapture {
        frames,
        ready,
        mut worker,
    } = prysm_capture::DeviceCapture::open(path.to_str(), width, height, *rate)
        .map_err(|error| ControlError::Adapter(Box::new(error)))?;
    let stop = worker.cancellation();
    let finished = async move {
        match worker.join().await {
            Ok(exit) => Completion {
                failure: exit
                    .failure
                    .map(|error| Failure::from(ControlError::Adapter(Box::new(error)))),
                cleanup_complete: true,
            },
            Err(error) => Completion {
                failure: Some(ControlError::RecoveryRequired(Box::new(error)).into()),
                cleanup_complete: false,
            },
        }
    }
    .boxed();
    Ok(Run {
        ready,
        frames,
        stop,
        finished,
    })
}
