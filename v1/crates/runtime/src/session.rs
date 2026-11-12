use crate::worker::Worker;
use prysm_core::{Config, EdgeColors, Frame};
use prysm_ports::{Error, FrameSource, IdleMode, LatestSlot, LedOutput, Result, SessionPhase};
use prysm_processor::PrysmProcessor;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A quiet source must not let the device time out of live mode: the last
/// colors are rewritten at this interval.
const REFRESH: Duration = Duration::from_millis(500);

/// The latest processed frame, for surfaces that render what the LEDs show.
#[derive(Debug, Clone)]
pub struct Preview {
    pub frame: Frame,
    pub colors: EdgeColors,
}

/// How a session ended. Release and idle run on every exit path.
#[derive(Debug)]
pub struct SessionEnd {
    /// Why the loop stopped; `None` when it was asked to.
    pub failure: Option<Error>,
    pub released: Result<()>,
    pub idled: Result<()>,
}

impl SessionEnd {
    /// The first failure, in the order it happened.
    ///
    /// # Errors
    /// The loop failure, else the release failure, else the idle failure.
    pub fn result(self) -> Result<()> {
        match self.failure {
            Some(failure) => Err(failure),
            None => self.released.and(self.idled),
        }
    }
}

struct Control {
    config: LatestSlot<Config>,
    idle: Mutex<IdleMode>,
}

/// One dedicated thread owning a source, the processor and an output.
///
/// Loop: stop flag, pending config at the frame boundary, `next_frame`,
/// process, write, preview; a quiet tick rewrites the last colors. Then `source.close()` and `output.idle(mode)`.
pub struct Session {
    control: Arc<Control>,
    worker: Worker<SessionEnd>,
}

impl Session {
    /// # Errors
    /// The thread could not be spawned; the adapters are dropped unreleased.
    pub fn start(
        mut source: Box<dyn FrameSource>,
        mut output: Box<dyn LedOutput>,
        config: Config,
        preview: Arc<LatestSlot<Preview>>,
    ) -> Result<Self> {
        let control = Arc::new(Control {
            config: LatestSlot::new(),
            idle: Mutex::new(IdleMode::Off),
        });
        let shared = control.clone();
        let worker = Worker::spawn("prysm-session", move |flags| {
            let mut processor = PrysmProcessor::new(&config);
            let mut last: Option<(EdgeColors, Instant)> = None;
            let failure = flags.pump(&mut *source, |frame| {
                let Some(frame) = frame else {
                    if let Some((colors, written)) = &mut last
                        && written.elapsed() >= REFRESH
                    {
                        output.write(colors)?;
                        *written = Instant::now();
                    }
                    return Ok(());
                };
                if let Some(config) = shared.config.take() {
                    processor.configure(&config);
                }
                let shown = frame.clone();
                let colors = processor.process_frame(frame);
                output.write(&colors)?;
                last = Some((colors.clone(), Instant::now()));
                preview.offer(Preview {
                    frame: shown,
                    colors,
                });
                Ok(())
            });
            flags.set_phase(SessionPhase::ClosingSource);
            let released = source.close();
            flags.set_phase(SessionPhase::IdlingOutput);
            let mode = *shared.idle.lock().unwrap_or_else(PoisonError::into_inner);
            let idled = output.idle(mode);
            SessionEnd {
                failure,
                released,
                idled,
            }
        })?;
        Ok(Self { control, worker })
    }

    /// Apply processing settings at the next frame boundary.
    pub fn configure(&self, config: Config) {
        self.control.config.offer(config);
    }

    /// Wait for the session to end on its own (a failure). Cancel-safe.
    ///
    /// # Errors
    /// The thread panicked while releasing its adapters.
    pub async fn finished(&mut self) -> Result<SessionEnd> {
        self.worker.finished().await
    }

    /// Stop, release, idle with `mode` and join within `deadline`.
    ///
    /// # Errors
    /// [`Error::Faulted`] with the phase the thread is stuck in when the
    /// deadline passes (the thread is then abandoned), or [`Error::Panicked`].
    pub async fn stop(self, mode: IdleMode, deadline: Duration) -> Result<SessionEnd> {
        *self
            .control
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = mode;
        self.worker.stop(deadline).await.map_err(Error::Faulted)?
    }

    /// Join after [`Session::finished`] returned.
    pub async fn join(self) {
        self.worker.join().await;
    }
}
