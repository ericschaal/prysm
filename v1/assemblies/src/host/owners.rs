//! Concrete task ownership. Only the resource-free coordinator may be aborted.
use prysm_contract::{ControlError, OwnerStopped};
use prysm_output_terminal::TerminalShutdown;
use prysm_output_wled::WledShutdown;
use prysm_settings_file::FileSettingsShutdown;
use std::future::{Future, pending};
use tokio::{
    sync::oneshot,
    task::{JoinError, JoinHandle},
};

pub(super) struct Completion {
    pub result: Result<Result<OwnerStopped, ControlError>, JoinError>,
    pub expected: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Owner {
    Application,
    Capture,
    Processing,
    Output,
    Settings,
}
impl Owner {
    pub fn name(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Capture => "capture",
            Self::Processing => "processing",
            Self::Output => "output",
            Self::Settings => "settings",
        }
    }
}

pub(crate) struct Task {
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
    pub(super) fn request_stop(&mut self) {
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
    pub(super) async fn completion(&mut self) -> Completion {
        let result = match &mut self.join {
            Some(join) => join.await,
            None => pending().await,
        };
        // Retire only after await: a losing select branch must retain ownership.
        self.join.take();
        self.shutdown.take();
        Completion {
            result,
            expected: self.expected,
        }
    }
    pub(super) fn pending(&self) -> bool {
        self.join.is_some()
    }
}

pub(crate) struct Owners {
    pub application: Task,
    pub capture: Task,
    pub flow: Task,
    pub output: Task,
    pub output_shutdown: OutputShutdown,
    pub settings: Option<SettingsOwner>,
}
pub(crate) struct SettingsOwner {
    pub task: Task,
    pub shutdown: FileSettingsShutdown,
}

/// Memory-only repositories have no worker to join; keep their select branch inactive.
pub(super) async fn settings_completion(settings: &mut Option<SettingsOwner>) -> Completion {
    match settings {
        Some(settings) => settings.task.completion().await,
        None => pending().await,
    }
}
pub(crate) enum OutputShutdown {
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
    #[cfg(test)]
    pub(super) fn task(&mut self, owner: Owner) -> &mut Task {
        match owner {
            Owner::Application => &mut self.application,
            Owner::Capture => &mut self.capture,
            Owner::Processing => &mut self.flow,
            Owner::Output => &mut self.output,
            Owner::Settings => &mut self.settings.as_mut().expect("owned repository").task,
        }
    }
    pub(super) fn terminate(&mut self) {
        self.output_shutdown.request();
        self.stop_settings();
        self.application.abort_coordinator();
        self.capture.request_stop();
        self.flow.request_stop();
        self.output.request_stop();
    }
    pub(super) fn stop_settings(&mut self) {
        if let Some(settings) = &mut self.settings {
            settings.shutdown.request();
            settings.task.request_stop();
        }
    }
    pub(super) fn pending(&self) -> Vec<&'static str> {
        let mut pending: Vec<_> = [
            ("application", &self.application),
            ("capture", &self.capture),
            ("processing", &self.flow),
            ("output", &self.output),
        ]
        .into_iter()
        .filter_map(|(name, task)| task.pending().then_some(name))
        .collect();
        if self
            .settings
            .as_ref()
            .is_some_and(|settings| settings.task.pending())
        {
            pending.push("settings");
        }
        pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;

    #[tokio::test]
    async fn completion_retains_pending_tasks_and_retires_once() {
        for request_stop in [false, true] {
            let (release, done) = oneshot::channel();
            let mut task = Task::spawn(|_| async {
                done.await.unwrap();
                Ok(OwnerStopped)
            });
            assert!(task.completion().now_or_never().is_none());
            assert!(task.pending());
            if request_stop {
                task.request_stop();
            }
            release.send(()).unwrap();
            let completion = task.completion().await;
            completion.result.unwrap().unwrap();
            assert_eq!(completion.expected, request_stop);
            assert!(!task.pending());
            assert!(task.completion().now_or_never().is_none());
        }
    }
}
