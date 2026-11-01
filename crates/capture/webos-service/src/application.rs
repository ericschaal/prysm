//! Service operations, filesystem transactions, and process execution.
use crate::{
    config::Config,
    files::Files,
    supervisor::{Action, Supervisor},
    worker::Worker,
};
use anyhow::{Result, ensure};
use prysm_webos_protocol::worker::{VideoAvailability, VideoUpdate};
use serde_json::{Value, json};
use std::{
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
    Start,
    Stop,
    Cleanup,
}
struct Scan {
    child: Child,
    deadline: Instant,
}
impl Drop for Scan {
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
    scan: Option<Scan>,
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
            self.pending = None;
            self.scan = None;
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
                self.files.persist(&config)?;
                if let Err(error) = self.files.autostart(config.autostart) {
                    if self.state.config.receiver.is_none() {
                        Files::remove(&self.files.root.join("config.json"))?;
                    } else {
                        self.files.persist(&self.state.config)?;
                    }
                    return Err(error);
                }
                self.state.configure(config, now);
            }
            AppRequest::Cleanup => {
                self.state.enabled = false;
                let mut config = self.state.config.clone();
                config.autostart = false;
                self.files.autostart(false)?;
                if config.receiver.is_some() {
                    self.files.persist(&config)?;
                }
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
    fn begin_scan(&mut self, now: Instant) -> Result<()> {
        let child = Command::new(&self.files.scanner)
            .arg("scan-services")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        self.scan = Some(Scan {
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
                self.scan = None;
                self.pending
                    .take()
                    .map(|_| json!({"returnValue":false,"errorText":format!("{error:#}")}))
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
            self.scan = None;
            ensure!(!timed_out, "Luna service rescan timed out");
            ensure!(exit.success(), "Luna service rescan failed: {exit}");
            ensure!(!self.shutting_down, "Service is shutting down");
            if matches!(self.pending, Some(Pending::Start)) && !self.shutting_down {
                self.state.start(now)?;
                self.execute(now)?;
            }
            self.pending = None;
            return Ok(Some(self.state.snapshot()));
        }
        if self.state.running {
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
        if matches!(self.pending, Some(Pending::Start))
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
