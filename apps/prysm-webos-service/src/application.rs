//! Service operations, filesystem transactions, and process execution.
use crate::{
    config::Config,
    files::Files,
    supervisor::{Action, Supervisor},
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
    SetConfig(Config),
    Cleanup,
}

enum Pending {
    Verify { config: Config, resume: bool },
    Start,
    Stop,
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
    scan: Option<Helper>,
    shutting_down: bool,
}
impl App {
    pub fn new(files: Files) -> Result<Self> {
        Ok(Self {
            state: Supervisor::new(files.load()?),
            files,
            worker: None,
            video: VideoUpdate {
                availability: VideoAvailability::Unavailable,
                reset: false,
            },
            pending: None,
            scan: None,
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
            self.abandon_pending(now);
        }
        result
    }
    fn request_inner(&mut self, request: AppRequest, now: Instant) -> Result<Option<Value>> {
        match request {
            AppRequest::Start => {
                self.files.manifest(true)?;
                self.begin_scan(now)?;
                self.pending = Some(Pending::Start);
            }
            AppRequest::Stop => {
                self.state.enabled = false;
                self.state.stop(now, true);
                self.pending = Some(Pending::Stop);
            }
            AppRequest::SetConfig(config) => {
                config.validate()?;
                if config.instance_url == self.state.config.instance_url {
                    self.save(config, now)?;
                } else {
                    self.pending = Some(Pending::Verify {
                        config,
                        resume: self.state.enabled,
                    });
                    self.state.enabled = false;
                    self.state.stop(now, true);
                }
            }
            AppRequest::Cleanup => {
                let mut config = self.state.config.clone();
                config.autostart = false;
                if config.instance_url.is_some() {
                    self.files.persist(&config)?;
                } else {
                    self.files.autostart(false)?;
                }
                self.state.enabled = false;
                self.state.config = config;
                self.state.stop(now, true);
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
    fn save(&mut self, config: Config, now: Instant) -> Result<()> {
        self.files.persist(&config)?;
        self.state.configure(config, now);
        Ok(())
    }

    fn abandon_pending(&mut self, now: Instant) {
        if let Some(Pending::Verify { resume, .. }) = self.pending.take() {
            self.state.enabled = resume && !self.shutting_down;
            self.state.reconcile(now);
        }
        self.scan = None;
    }
    fn begin_scan(&mut self, now: Instant) -> Result<()> {
        let child = Command::new(&self.files.scanner)
            .arg("scan-services")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        self.scan = Some(Helper {
            child,
            deadline: now + Duration::from_secs(10),
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
                Action::Spawn => match Worker::spawn(&self.files.worker, &self.state.config) {
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
        match self.advance(now) {
            Ok(value) => value,
            Err(error) => {
                self.state.error = Some(format!("{error:#}"));
                let pending = self.pending.is_some();
                self.abandon_pending(now);
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
                match worker.update_video(self.video) {
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
                            Ok(event) => self.state.event(event, now),
                            Err(error) if !self.state.stopping => self.state.failed(error, now),
                            Err(_) => (),
                        }
                    }
                    if let Some(log) = update.log {
                        self.state.log(log);
                    }
                    if let Some(exit) = update.exit {
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
        if let Some(scan) = &mut self.scan {
            if now >= scan.deadline {
                scan.child.kill()?;
            }
            let Some(exit) = scan.child.try_wait()? else {
                return Ok(None);
            };
            let timed_out = now >= scan.deadline;
            if let Some(Pending::Verify { config, resume }) = &self.pending {
                ensure!(!self.shutting_down, "Service is shutting down");
                if timed_out || !exit.success() {
                    let mut detail = String::new();
                    if let Some(mut err) = scan.child.stderr.take() {
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
                    let mut reply = prysm_webos_service::contract::ErrorReply::new(message);
                    reply.retry_default_port = timed_out || exit.code() == Some(2);
                    self.abandon_pending(now);
                    self.execute(now)?;
                    return Ok(Some(serde_json::to_value(reply)?));
                }
                self.state.enabled = *resume;
                self.save(config.clone(), now)?;
                self.pending = None;
                self.scan = None;
                self.execute(now)?;
                return Ok(Some(self.state.snapshot()));
            }
            self.scan = None;
            ensure!(!timed_out, "Luna service rescan timed out");
            ensure!(exit.success(), "Luna service rescan failed: {exit}");
            if matches!(self.pending, Some(Pending::Start)) {
                ensure!(!self.shutting_down, "Service is shutting down");
                self.state.start(now)?;
                self.execute(now)?;
            }
            self.pending = None;
            return Ok(Some(self.state.snapshot()));
        }
        if self.state.running {
            return Ok(None);
        }
        if let Some(Pending::Verify { config, .. }) = &self.pending {
            ensure!(!self.shutting_down, "Service is shutting down");
            let child = Command::new(&self.files.worker)
                .arg("--verify-instance")
                .arg(
                    config
                        .instance_url
                        .as_ref()
                        .expect("validated origin")
                        .to_string(),
                )
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()?;
            self.scan = Some(Helper {
                child,
                deadline: now + Duration::from_secs(12),
            });
            return Ok(None);
        }
        if matches!(self.pending, Some(Pending::Cleanup)) {
            self.files.manifest(false)?;
            self.begin_scan(now)?;
            return Ok(None);
        }
        Ok(self.pending.take().map(|_| self.state.snapshot()))
    }
    pub fn shutdown(&mut self, now: Instant) {
        self.shutting_down = true;
        if !matches!(self.pending, Some(Pending::Cleanup))
            && let Some(scan) = &mut self.scan
        {
            let _ = scan.child.kill();
        }
        self.state.enabled = false;
        self.state.stop(now, true);
    }
    pub fn stopped(&self) -> bool {
        !self.state.running && self.scan.is_none()
    }
}
