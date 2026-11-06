//! Concrete task ownership. Only the resource-free coordinator may be aborted.
use prysm_contract::{ControlError, OwnerStopped};
use prysm_output_terminal::TerminalShutdown;
use prysm_output_wled::WledShutdown;
use std::future::{Future, pending};
use tokio::{
    sync::oneshot,
    task::{JoinError, JoinHandle},
};

pub(super) type Completion = Result<Result<OwnerStopped, ControlError>, JoinError>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Owner {
    Application,
    Capture,
    Processing,
    Output,
}
impl Owner {
    pub fn name(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Capture => "capture",
            Self::Processing => "processing",
            Self::Output => "output",
        }
    }
}

pub(super) struct Task {
    join: Option<JoinHandle<Result<OwnerStopped, ControlError>>>,
    shutdown: Option<oneshot::Sender<()>>,
    expected: bool,
}
impl Task {
    pub fn spawn<F: Future<Output = Result<OwnerStopped, ControlError>> + Send + 'static>(
        run: impl FnOnce(futures::future::BoxFuture<'static, ()>) -> F,
    ) -> Self {
        let (shutdown, stopped) = oneshot::channel();
        let stopped = Box::pin(async {
            let _ = stopped.await;
        });
        Self {
            join: Some(tokio::spawn(run(stopped))),
            shutdown: Some(shutdown),
            expected: false,
        }
    }
    pub fn request_stop(&mut self) {
        if self.join.is_some() {
            self.expected = true;
        }
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
    fn abort_coordinator(&mut self) {
        self.expected = true;
        if let Some(join) = &self.join {
            join.abort();
        }
    }
    pub async fn completion(&mut self) -> Completion {
        match &mut self.join {
            Some(join) => join.await,
            None => pending().await,
        }
    }
    pub fn take_completed(&mut self) -> bool {
        self.join.take().expect("completed owner was retained");
        self.shutdown.take();
        self.expected
    }
    pub fn pending(&self) -> bool {
        self.join.is_some()
    }
}

pub(super) struct Owners {
    pub application: Task,
    pub capture: Task,
    pub flow: Task,
    pub output: Task,
    pub output_shutdown: OutputShutdown,
}
pub(super) enum OutputShutdown {
    Wled(WledShutdown),
    Terminal(TerminalShutdown),
}
impl From<WledShutdown> for OutputShutdown {
    fn from(value: WledShutdown) -> Self {
        Self::Wled(value)
    }
}
impl From<TerminalShutdown> for OutputShutdown {
    fn from(value: TerminalShutdown) -> Self {
        Self::Terminal(value)
    }
}
impl OutputShutdown {
    fn request(&self) {
        match self {
            Self::Wled(control) => control.request(),
            Self::Terminal(control) => control.request(),
        }
    }
}
impl Owners {
    pub fn task(&mut self, owner: Owner) -> &mut Task {
        match owner {
            Owner::Application => &mut self.application,
            Owner::Capture => &mut self.capture,
            Owner::Processing => &mut self.flow,
            Owner::Output => &mut self.output,
        }
    }
    pub fn terminate(&mut self) {
        self.output_shutdown.request();
        self.application.abort_coordinator();
        self.capture.request_stop();
        self.flow.request_stop();
        self.output.request_stop();
    }
    pub fn pending(&self) -> Vec<&'static str> {
        [
            ("application", &self.application),
            ("capture", &self.capture),
            ("processing", &self.flow),
            ("output", &self.output),
        ]
        .into_iter()
        .filter_map(|(name, task)| task.pending().then_some(name))
        .collect()
    }
}
