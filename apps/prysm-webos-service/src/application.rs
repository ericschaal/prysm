//! Service operations, filesystem transactions, and process execution.
use crate::{
    config::{Config, ProcessingLocation},
    files::Files,
    supervisor::{Action, Event, Selection, Supervisor},
    worker::Worker,
};
use anyhow::{Result, ensure};
use prysm_webos_protocol::worker::{VideoAvailability, VideoUpdate};
use serde_json::Value;
use std::{
    io::Read as _,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(all(test, not(miri)))]
pub(crate) mod tests;

#[derive(Debug)]
pub enum AppRequest {
    Start,
    Stop,
    SetConfig(Box<Config>),
    Cleanup,
}

/// The pending operation owns its subprocess until completion or cancellation.
enum Pending {
    Pause {
        progress: PauseProgress,
        deadline: Instant,
    },
    Stop,
    Cleanup,
    Helper {
        operation: Operation,
        process: Helper,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum PauseProgress {
    Send,
    AwaitRelease,
    Released,
}

enum Operation {
    Verify(Box<Selection>),
    SelectEmbedded(Box<Selection>),
    Start,
    Cleanup,
}
struct Helper {
    child: Child,
    deadline: Instant,
}
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        crate::worker::reap(&mut self.child);
    }
}
pub struct App {
    pub state: Supervisor,
    files: Files,
    worker: Option<Worker>,
    video: VideoUpdate,
    pending: Option<Pending>,
    shutting_down: bool,
}
impl App {
    pub fn new(files: Files) -> Result<Self> {
        let mut state = Supervisor::new(files.load()?);
        if let Err(error) = files.autostart(state.config.needs_boot_hook()) {
            state.error = Some(format!("Restoring boot hook: {error:#}"));
        }
        Ok(Self {
            state,
            files,
            worker: None,
            video: VideoUpdate {
                availability: VideoAvailability::Unavailable,
                reset: false,
            },
            pending: None,
            shutting_down: false,
        })
    }
    pub fn authorize(&self, sender: &str) -> Result<()> {
        ensure!(
            sender == self.files.app_id,
            "Only the Prysm Capture app may change service state"
        );
        Ok(())
    }
    pub fn request(&mut self, request: AppRequest, now: Instant) -> Result<Option<Value>> {
        ensure!(
            self.pending.is_none() && !self.shutting_down,
            "A service change is already in progress"
        );
        let result = self.request_inner(request, now);
        if result.is_err() {
            self.pending = None;
        }
        result
    }
    fn request_inner(&mut self, request: AppRequest, now: Instant) -> Result<Option<Value>> {
        match request {
            AppRequest::Start => {
                self.files.manifest(true)?;
                self.begin_scan(Operation::Start, now)?;
            }
            AppRequest::Stop => {
                self.state.pause_capture(now);
                self.pending = Some(if self.state.hosts_portal() {
                    Pending::Pause {
                        progress: PauseProgress::Send,
                        deadline: now + Duration::from_secs(5),
                    }
                } else {
                    Pending::Stop
                });
            }
            AppRequest::SetConfig(config) => {
                let selected = Selection::try_from(*config)?;
                if let Some(url) = selected.receiver_to_verify(&self.state.config) {
                    // Verification opens a receiver connection; it never owns the capture tap.
                    let child = Command::new(&self.files.worker)
                        .arg("--verify-instance")
                        .arg(url.to_string())
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::piped())
                        .spawn()?;
                    self.pending = Some(Pending::Helper {
                        operation: Operation::Verify(Box::new(selected)),
                        process: Helper {
                            child,
                            deadline: now + Duration::from_secs(12),
                        },
                    });
                } else if selected.preferences().location == ProcessingLocation::Embedded
                    && !self.state.hosts_portal()
                {
                    self.files.manifest(true)?;
                    self.begin_scan(Operation::SelectEmbedded(Box::new(selected)), now)?;
                } else {
                    self.save(selected, now)?;
                }
            }
            AppRequest::Cleanup => {
                let mut config = self.state.config.clone();
                config.autostart = false;
                let removing_portal = config.location == ProcessingLocation::Embedded;
                config.location = ProcessingLocation::Remote;
                if removing_portal
                    || config.instance_url.is_some()
                    || self.files.has_versioned_config()?
                {
                    self.files.persist(&config)?;
                } else {
                    self.files.autostart(false)?;
                }
                self.state.config = config;
                self.state.deactivate(now);
                self.pending = Some(Pending::Cleanup);
            }
        }
        self.execute(now)?;
        if self.pending.is_none() {
            Ok(Some(self.state.snapshot()))
        } else {
            self.finish(now)
        }
    }
    fn save(&mut self, selected: Selection, now: Instant) -> Result<()> {
        self.files.persist(selected.preferences())?;
        self.state.configure(selected, now);
        Ok(())
    }

    fn begin_scan(&mut self, operation: Operation, now: Instant) -> Result<()> {
        let child = Command::new(&self.files.scanner)
            .arg("scan-services")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        self.pending = Some(Pending::Helper {
            operation,
            process: Helper {
                child,
                deadline: now + Duration::from_secs(10),
            },
        });
        Ok(())
    }
    pub fn video_available(&mut self, available: VideoAvailability) {
        self.video.observe(available);
    }
    pub fn reset_video(&mut self) {
        self.video.reset = true;
    }
    pub fn power(&mut self, power: Option<String>, error: Option<String>, now: Instant) {
        if let Some(error) = error {
            self.state.error = Some(error);
        }
        self.state.set_power(power, now);
    }
    fn execute(&mut self, now: Instant) -> Result<()> {
        for action in self.state.actions() {
            match action {
                Action::Spawn(target) => match Worker::spawn(
                    &self.files.worker,
                    &target,
                    &self.files.portal_settings_path(),
                ) {
                    Ok(worker) => self.worker = Some(worker),
                    Err(error) => {
                        self.state.error = Some(format!("{error:#}"));
                        self.state.closed(false, "spawn failed", now);
                    }
                },
                Action::Terminate => {
                    if let Some(worker) = &mut self.worker {
                        worker.terminate()?;
                    }
                }
                Action::Kill => {
                    if let Some(worker) = &mut self.worker {
                        worker.kill()?;
                    }
                }
            }
        }
        Ok(())
    }
    pub fn tick(&mut self, now: Instant) -> Option<Value> {
        let pending = self.pending.is_some();
        let verifying = matches!(
            self.pending,
            Some(Pending::Helper {
                operation: Operation::Verify(_) | Operation::SelectEmbedded(_),
                ..
            })
        );
        match self.advance(now) {
            Ok(value) => value,
            Err(error) => {
                if !verifying {
                    self.state.error = Some(format!("{error:#}"));
                }
                self.pending = None;
                pending.then(|| {
                    serde_json::to_value(prysm_webos_service::contract::ErrorReply::new(format!(
                        "{error:#}"
                    )))
                    .expect("serializable service error")
                })
            }
        }
    }
    fn advance(&mut self, now: Instant) -> Result<Option<Value>> {
        if let Some(worker) = &mut self.worker {
            if !self.state.stopping {
                let video = if self.state.capture_allowed() {
                    self.video
                } else {
                    VideoUpdate {
                        availability: VideoAvailability::Unavailable,
                        reset: false,
                    }
                };
                let delivery = match &mut self.pending {
                    Some(Pending::Pause { progress, .. }) if *progress == PauseProgress::Send => {
                        worker.request_pause().inspect(|sent| {
                            if *sent {
                                *progress = PauseProgress::AwaitRelease;
                            }
                        })
                    }
                    _ => worker.update_video(video),
                };
                match delivery {
                    Ok(true) => self.video.reset = false,
                    Ok(false) => (),
                    Err(error) => self.state.failed(format!("worker control: {error:#}"), now),
                }
            }
            match worker.poll() {
                Err(error) => self.state.failed(format!("{error:#}"), now),
                Ok(update) => {
                    for event in update.events {
                        match event {
                            Ok(event) => {
                                if event == Event::CapturePaused
                                    && let Some(Pending::Pause {
                                        progress: progress @ PauseProgress::AwaitRelease,
                                        ..
                                    }) = &mut self.pending
                                {
                                    *progress = PauseProgress::Released;
                                }
                                self.state.event(event, now);
                            }
                            Err(error) if !self.state.stopping => self.state.failed(error, now),
                            Err(_) => (),
                        }
                    }
                    if let Some(log) = update.log {
                        self.state.log(log);
                    }
                    if let Some(exit) = update.exit {
                        if let Some(Pending::Pause { progress, .. }) = &mut self.pending {
                            *progress = PauseProgress::Released;
                        }
                        self.worker = None;
                        self.state.closed(exit.success(), &exit.to_string(), now);
                    }
                }
            }
        }
        self.state.tick(now);
        self.execute(now)?;
        self.finish(now)
    }
    fn finish(&mut self, now: Instant) -> Result<Option<Value>> {
        let Some(pending) = self.pending.take() else {
            return Ok(None);
        };
        match pending {
            Pending::Pause { progress, deadline }
                if self.worker.is_some() && progress != PauseProgress::Released =>
            {
                if now >= deadline {
                    self.state
                        .failed("Timed out pausing native capture".into(), now);
                    anyhow::bail!("Timed out pausing native capture");
                }
                self.pending = Some(Pending::Pause { progress, deadline });
                return Ok(None);
            }
            Pending::Stop | Pending::Cleanup if self.state.running => {
                self.pending = Some(pending);
                return Ok(None);
            }
            Pending::Stop | Pending::Pause { .. } => (),
            Pending::Cleanup => {
                self.files.manifest(false)?;
                self.begin_scan(Operation::Cleanup, now)?;
                return Ok(None);
            }
            Pending::Helper {
                operation,
                process: mut helper,
            } => {
                let timed_out = now >= helper.deadline;
                if timed_out {
                    helper.child.kill()?;
                }
                let Some(exit) = helper.child.try_wait()? else {
                    self.pending = Some(Pending::Helper {
                        operation,
                        process: helper,
                    });
                    return Ok(None);
                };
                if !matches!(operation, Operation::Verify(_)) {
                    ensure!(!timed_out, "Luna service rescan timed out");
                    ensure!(exit.success(), "Luna service rescan failed: {exit}");
                }
                match operation {
                    Operation::Verify(selected) => {
                        ensure!(!self.shutting_down, "Service is shutting down");
                        if timed_out || !exit.success() {
                            let mut detail = String::new();
                            if let Some(mut err) = helper.child.stderr.take() {
                                err.read_to_string(&mut detail)?;
                            }
                            let message = if timed_out {
                                "Prysm connection timed out".to_owned()
                            } else {
                                format!(
                                    "Could not verify Prysm: {}",
                                    detail.trim().trim_start_matches("Error: ")
                                )
                            };
                            let reply = prysm_webos_service::contract::ErrorReply::new(message);
                            return Ok(Some(serde_json::to_value(reply)?));
                        }
                        self.save(*selected, now)?;
                    }
                    Operation::SelectEmbedded(selected) => {
                        ensure!(!self.shutting_down, "Service is shutting down");
                        self.save(*selected, now)?;
                    }
                    Operation::Start => {
                        ensure!(!self.shutting_down, "Service is shutting down");
                        self.state.start(now)?;
                    }
                    Operation::Cleanup => (),
                }
                self.execute(now)?;
            }
        }
        Ok(Some(self.state.snapshot()))
    }
    pub fn shutdown(&mut self, now: Instant) {
        self.shutting_down = true;
        if let Some(Pending::Helper {
            operation,
            process: helper,
        }) = &mut self.pending
            && !matches!(operation, Operation::Cleanup)
        {
            let _ = helper.child.kill();
        }
        self.state.deactivate(now);
    }
    pub fn stopped(&self) -> bool {
        !self.state.running && self.pending.is_none()
    }
}
